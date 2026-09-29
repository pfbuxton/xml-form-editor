// The extension host side of the XML Form Editor. It opens XML files in the form, gives the
// webview (crates/xml-form-editor-ui) the document, the files it links to and the schemas they
// name, and applies the edits the webview sends back. Everything else, from reading the schemas to
// computing the edits, is Rust.

import * as vscode from "vscode";

const VIEW_TYPE = "xmlFormEditor.form";

/** The forms that are open. */
const sessions = new Set<FormSession>();

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.window.registerCustomEditorProvider(VIEW_TYPE, new FormEditorProvider(context.extensionUri), {
      // Keeps the form's collapsed sections, filter and scroll position while its tab is hidden
      webviewOptions: { retainContextWhenHidden: true },
      supportsMultipleEditorsPerDocument: true,
    }),
    vscode.commands.registerCommand("xmlFormEditor.openForm", (uri?: vscode.Uri) =>
      openForm(uri, vscode.ViewColumn.Active),
    ),
    vscode.commands.registerCommand("xmlFormEditor.openFormToSide", (uri?: vscode.Uri) =>
      openForm(uri, vscode.ViewColumn.Beside),
    ),
    vscode.commands.registerCommand("xmlFormEditor.openSource", (uri?: vscode.Uri) => openSource(uri)),
    vscode.commands.registerCommand("xmlFormEditor.save", () => saveActiveForm()),
  );
}

export function deactivate(): void {}

async function openForm(uri: vscode.Uri | undefined, column: vscode.ViewColumn): Promise<void> {
  const target = uri ?? activeUri();
  if (!target) {
    void vscode.window.showInformationMessage("Open an XML file first, then open it in the form editor.");
    return;
  }
  await vscode.commands.executeCommand("vscode.openWith", target, VIEW_TYPE, column);
}

async function openSource(uri: vscode.Uri | undefined): Promise<void> {
  const target = uri ?? activeUri();
  if (target) {
    await vscode.window.showTextDocument(target, { viewColumn: vscode.ViewColumn.Beside, preview: false });
  }
}

/** Ctrl+S in a form: saves the file and the files it links to, as the form's Save button does. */
async function saveActiveForm(): Promise<void> {
  const session = [...sessions].find((s) => s.active);
  if (session) {
    await session.saveAll();
  } else {
    await vscode.commands.executeCommand("workbench.action.files.save");
  }
}

/** The file in the active tab, whether it's open as text or in the form. */
function activeUri(): vscode.Uri | undefined {
  const input = vscode.window.tabGroups.activeTabGroup.activeTab?.input;
  if (input instanceof vscode.TabInputText || input instanceof vscode.TabInputCustom) {
    return input.uri;
  }
  return vscode.window.activeTextEditor?.document.uri;
}

type TextEdit = { start: number; end: number; text: string };

/**
 * The messages the webview sends; see `Out` in crates/xml-form-editor-ui/src/bridge.rs. A
 * `document` is a file the form shows, by its URI: the one it was opened on, or one it links to.
 */
type FromWebview =
  | { type: "ready" }
  | { type: "requestSchema"; document: string; location: string }
  | { type: "openLink"; document: string; href: string }
  | { type: "edit"; document: string; id: number; version: number; edits: TextEdit[] }
  | { type: "reveal"; document: string; offset: number }
  | { type: "openSource" }
  | { type: "openSchema"; document: string }
  | { type: "save" }
  | { type: "copy"; text: string };

class FormEditorProvider implements vscode.CustomTextEditorProvider {
  constructor(private readonly extensionUri: vscode.Uri) {}

  resolveCustomTextEditor(document: vscode.TextDocument, panel: vscode.WebviewPanel): void {
    const webviewRoot = vscode.Uri.joinPath(this.extensionUri, "dist", "webview");
    const mediaRoot = vscode.Uri.joinPath(this.extensionUri, "media");
    panel.webview.options = { enableScripts: true, localResourceRoots: [webviewRoot, mediaRoot] };
    panel.webview.html = webviewHtml(panel.webview, webviewRoot, mediaRoot);
    const session = new FormSession(document, panel);
    sessions.add(session);
    panel.onDidDispose(() => {
      sessions.delete(session);
      session.dispose();
    });
  }
}

/** A file the form shows. */
type Shown = {
  uri: vscode.Uri;
  /** The text last sent to the form, which its edits are made against */
  sent?: string;
  watcher?: vscode.FileSystemWatcher;
};

/** The schema a file names. */
type SchemaFile = { location: string; uri: vscode.Uri; watcher: vscode.FileSystemWatcher };

/**
 * Connects one form to its document, to the files the document links to, which the form shows in
 * place, and to the schemas they all name.
 */
class FormSession implements vscode.Disposable {
  private readonly disposables: vscode.Disposable[] = [];
  /** The files the form shows, by URI: the document, and the files it links to */
  private readonly files = new Map<string, Shown>();
  /** The schema each file names, by the file's URI */
  private readonly schemas = new Map<string, SchemaFile>();
  /** The linked files the form has changed, by URI */
  private readonly changed = new Set<string>();

  constructor(
    private readonly document: vscode.TextDocument,
    private readonly panel: vscode.WebviewPanel,
  ) {
    this.show(document.uri);
    this.disposables.push(
      panel.webview.onDidReceiveMessage((message: FromWebview) => void this.receive(message)),
      vscode.workspace.onDidChangeTextDocument((event) => {
        if (this.files.has(event.document.uri.toString())) {
          // Also sent when only whether it's dirty changes, as after undoing back to the saved text
          this.sendDocument(event.document);
        }
        if (event.contentChanges.length > 0) {
          for (const [file, schema] of this.schemas) {
            if (sameUri(event.document.uri, schema.uri)) {
              void this.sendSchema(file);
            }
          }
        }
      }),
      vscode.workspace.onDidSaveTextDocument((saved) => {
        if (this.files.has(saved.uri.toString())) {
          this.post({ type: "saved", uri: saved.uri.toString(), text: saved.getText() });
          this.sendDocument(saved);
        }
      }),
      // VS Code closes files that no editor shows, and counts their versions afresh when it opens
      // them again
      vscode.workspace.onDidOpenTextDocument((opened) => {
        if (this.files.has(opened.uri.toString())) {
          this.sendDocument(opened);
        }
      }),
    );
  }

  /** Whether the form is the active editor. */
  get active(): boolean {
    return this.panel.active;
  }

  dispose(): void {
    const unsaved = vscode.workspace.textDocuments.filter((d) => d.isDirty && this.changed.has(d.uri.toString()));
    this.files.forEach((file) => file.watcher?.dispose());
    this.schemas.forEach((schema) => schema.watcher.dispose());
    this.disposables.forEach((d) => d.dispose());
    if (unsaved.length > 0) {
      void offerToSave(unsaved);
    }
  }

  /** Saves the file and the files it links to, those with unsaved changes. */
  async saveAll(): Promise<void> {
    for (const { uri } of this.files.values()) {
      const document = vscode.workspace.textDocuments.find((d) => sameUri(d.uri, uri));
      if (document?.isDirty && !(await document.save())) {
        void vscode.window.showErrorMessage(`Could not save ${displayPath(uri)}.`);
      }
    }
  }

  private post(message: object): void {
    void this.panel.webview.postMessage(message);
  }

  /** Starts following a file the form shows. */
  private show(uri: vscode.Uri): void {
    if (this.files.has(uri.toString())) {
      return;
    }
    const file: Shown = { uri };
    // The file changing on disk by other means, such as git
    if (uri.scheme !== "untitled") {
      file.watcher = vscode.workspace.createFileSystemWatcher(
        new vscode.RelativePattern(vscode.Uri.joinPath(uri, ".."), basename(uri)),
      );
      file.watcher.onDidChange(() => void this.changedOnDisk(uri));
    }
    this.files.set(uri.toString(), file);
  }

  private async changedOnDisk(uri: vscode.Uri): Promise<void> {
    await this.sendSaved(uri, true);
    // An open document reloads, and sends its change; one that VS Code has closed is read again
    if (!vscode.workspace.textDocuments.some((d) => sameUri(d.uri, uri))) {
      this.sendDocument(await vscode.workspace.openTextDocument(uri));
    }
  }

  private sendDocument(document: vscode.TextDocument): void {
    const uri = document.uri.toString();
    const text = document.getText();
    const file = this.files.get(uri);
    if (file) {
      file.sent = text;
    }
    this.post({
      type: "document",
      uri,
      main: sameUri(document.uri, this.document.uri),
      text,
      version: document.version,
      dirty: document.isDirty,
    });
  }

  /**
   * Tells the form what a file holds on disk, which it compares values with to mark those that
   * changed. It's told explicitly, as whether a document is dirty can lag behind its changes.
   */
  private async sendSaved(uri: vscode.Uri, fromDisk: boolean): Promise<void> {
    try {
      const text = fromDisk
        ? new TextDecoder().decode(await vscode.workspace.fs.readFile(uri))
        : (await vscode.workspace.openTextDocument(uri)).getText();
      this.post({ type: "saved", uri: uri.toString(), text });
    } catch {
      // Not on disk (yet): the form marks changes from the first save on
    }
  }

  private async receive(message: FromWebview): Promise<void> {
    switch (message.type) {
      case "ready":
        await this.sendSaved(this.document.uri, this.document.isDirty);
        this.sendDocument(this.document);
        break;
      case "save":
        await this.saveAll();
        break;
      case "copy":
        // VS Code's clipboard is the local machine's, even in a remote window
        await vscode.env.clipboard.writeText(message.text);
        break;
      case "requestSchema":
        await this.loadSchema(message.document, message.location);
        break;
      case "openLink":
        await this.openLink(message.document, message.href);
        break;
      case "edit":
        await this.applyEdit(message.document, message.id, message.version, message.edits);
        break;
      case "reveal":
        await this.reveal(message.document, message.offset);
        break;
      case "openSource":
        await this.showText(this.document);
        break;
      case "openSchema": {
        const schema = this.schemas.get(message.document);
        if (schema) {
          await vscode.window.showTextDocument(schema.uri, { viewColumn: vscode.ViewColumn.Beside, preview: false });
        }
        break;
      }
    }
  }

  /** Opens a file that a file the form shows links to, and sends it to the form. */
  private async openLink(from: string, href: string): Promise<void> {
    const base = this.files.get(from)?.uri;
    if (!base) {
      return;
    }
    const answer = (result: { uri: string } | { error: string }) =>
      this.post({ type: "link", document: from, href, ...result });
    const target = resolveLocation(base, href);
    if (!target) {
      answer({ error: `${href} is not a local file, and only local files are shown.` });
      return;
    }
    try {
      await vscode.workspace.fs.stat(target);
    } catch {
      answer({ error: `${href} was not found: there's no ${displayPath(target)}` });
      return;
    }
    let document: vscode.TextDocument;
    try {
      document = await vscode.workspace.openTextDocument(target);
    } catch (e) {
      answer({ error: `Could not read ${displayPath(target)}: ${messageOf(e)}` });
      return;
    }
    this.show(document.uri);
    answer({ uri: document.uri.toString() });
    await this.sendSaved(document.uri, document.isDirty);
    this.sendDocument(document);
  }

  private async applyEdit(file: string, id: number, version: number, edits: TextEdit[]): Promise<void> {
    let applied = false;
    let error: string | undefined;
    let document: vscode.TextDocument | undefined;
    try {
      const shown = this.files.get(file);
      if (!shown) {
        throw new Error("the form doesn't show that file");
      }
      document = await vscode.workspace.openTextDocument(shown.uri);
      // Edits made against another version would land in the wrong place. Refused, the form makes
      // them again against the new text. The text is compared too, as a file that VS Code closed
      // and opened again counts its versions afresh.
      if (version === document.version && document.getText() === shown.sent) {
        const edit = new vscode.WorkspaceEdit();
        for (const { start, end, text } of edits) {
          const range = new vscode.Range(document.positionAt(start), document.positionAt(end));
          edit.replace(document.uri, range, text);
        }
        applied = await vscode.workspace.applyEdit(edit);
        if (!applied) {
          error = "VS Code refused the edit. Is the file read-only?";
        } else if (!sameUri(document.uri, this.document.uri)) {
          this.changed.add(file);
        }
      }
    } catch (e) {
      error = messageOf(e);
    } finally {
      // The form reads the answer against the document it holds, so that goes first
      if (document) {
        this.sendDocument(document);
      }
      this.post({ type: "editDone", id, applied, error });
    }
  }

  /** Shows a document as text: in the editor already showing it, or in a new one beside the form. */
  private async showText(document: vscode.TextDocument): Promise<vscode.TextEditor> {
    const visible = vscode.window.visibleTextEditors.find((editor) => sameUri(editor.document.uri, document.uri));
    return vscode.window.showTextDocument(document, {
      viewColumn: visible?.viewColumn ?? vscode.ViewColumn.Beside,
      preview: false,
    });
  }

  private async reveal(file: string, offset: number): Promise<void> {
    const shown = this.files.get(file);
    if (!shown) {
      return;
    }
    const document = await vscode.workspace.openTextDocument(shown.uri);
    const editor = await this.showText(document);
    const position = document.positionAt(offset);
    editor.selection = new vscode.Selection(position, position);
    editor.revealRange(new vscode.Range(position, position), vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  }

  private async loadSchema(file: string, location: string): Promise<void> {
    this.schemas.get(file)?.watcher.dispose();
    this.schemas.delete(file);
    const base = this.files.get(file)?.uri;
    if (!base) {
      return;
    }
    const uri = resolveLocation(base, location);
    if (!uri) {
      this.post({
        type: "schema",
        document: file,
        location,
        error: `${location} is not a local file, and only local schemas are supported.`,
      });
      return;
    }
    const watcher = vscode.workspace.createFileSystemWatcher(
      new vscode.RelativePattern(vscode.Uri.joinPath(uri, ".."), basename(uri)),
    );
    const reload = () => void this.sendSchema(file);
    watcher.onDidChange(reload);
    watcher.onDidCreate(reload);
    watcher.onDidDelete(reload);
    this.schemas.set(file, { location, uri, watcher });
    await this.sendSchema(file);
  }

  private async sendSchema(file: string): Promise<void> {
    const schema = this.schemas.get(file);
    if (!schema) {
      return;
    }
    const path = displayPath(schema.uri);
    const answer = (result: { text: string } | { error: string }) =>
      this.post({ type: "schema", document: file, location: schema.location, path, ...result });
    try {
      // If the schema is open, it may have unsaved changes
      const open = vscode.workspace.textDocuments.find((d) => sameUri(d.uri, schema.uri));
      answer({ text: open ? open.getText() : new TextDecoder().decode(await vscode.workspace.fs.readFile(schema.uri)) });
    } catch (e) {
      answer({ error: `Could not read the schema ${path}: ${messageOf(e)}` });
    }
  }
}

/** Offers to save the linked files that a form changed and didn't save, as it closes. */
async function offerToSave(documents: vscode.TextDocument[]): Promise<void> {
  const names = documents.map((d) => basename(d.uri)).join(", ");
  const verb = documents.length === 1 ? "is" : "are";
  const choice = await vscode.window.showWarningMessage(
    `The form you closed changed ${names}, which ${verb} not saved.`,
    "Save",
  );
  if (choice === "Save") {
    for (const document of documents) {
      await document.save();
    }
  }
}

/**
 * Resolves a location as a document writes it, such as a schema location or a link, to a file:
 * relative to the document, or absolute. `undefined` for remote ones.
 */
function resolveLocation(documentUri: vscode.Uri, location: string): vscode.Uri | undefined {
  if (/^[a-zA-Z]:[\\/]/.test(location) || location.startsWith("\\\\")) {
    return vscode.Uri.file(location);
  }
  const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(location)?.[1].toLowerCase();
  if (scheme === "file") {
    return vscode.Uri.parse(location);
  }
  if (scheme) {
    return undefined;
  }
  const path = decodeLocation(location).replace(/\\/g, "/");
  return path.startsWith("/") ? documentUri.with({ path }) : vscode.Uri.joinPath(documentUri, "..", path);
}

/** A location is a URI reference, so it may have escapes such as `%20`. */
function decodeLocation(location: string): string {
  try {
    return decodeURI(location);
  } catch {
    return location;
  }
}

/** A file's path as people read it. */
function displayPath(uri: vscode.Uri): string {
  return uri.scheme === "file" ? uri.fsPath : uri.toString();
}

function basename(uri: vscode.Uri): string {
  return uri.path.slice(uri.path.lastIndexOf("/") + 1);
}

function sameUri(a: vscode.Uri, b: vscode.Uri): boolean {
  return a.toString() === b.toString();
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function webviewHtml(webview: vscode.Webview, webviewRoot: vscode.Uri, mediaRoot: vscode.Uri): string {
  const nonce = makeNonce();
  const script = webview.asWebviewUri(vscode.Uri.joinPath(webviewRoot, "xml_form_editor_ui.js"));
  const wasm = webview.asWebviewUri(vscode.Uri.joinPath(webviewRoot, "xml_form_editor_ui_bg.wasm"));
  const style = webview.asWebviewUri(vscode.Uri.joinPath(mediaRoot, "webview.css"));
  const csp = [
    "default-src 'none'",
    `style-src ${webview.cspSource}`,
    // 'wasm-unsafe-eval' allows compiling WebAssembly, and nothing more
    `script-src 'nonce-${nonce}' ${webview.cspSource} 'wasm-unsafe-eval'`,
    // The WebAssembly module is fetched
    `connect-src ${webview.cspSource}`,
  ].join("; ");
  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="${csp}">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <link rel="stylesheet" href="${style}">
  <title>XML Form Editor</title>
</head>
<body>
  <script type="module" nonce="${nonce}">
    import init from "${script}";
    init({ module_or_path: "${wasm}" }).catch((error) => {
      document.body.textContent = "The form editor failed to load: " + error;
    });
  </script>
</body>
</html>`;
}

function makeNonce(): string {
  const characters = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  return Array.from({ length: 32 }, () => characters[Math.floor(Math.random() * characters.length)]).join("");
}
