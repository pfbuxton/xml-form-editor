// The extension host side of the XML Form Editor. It opens XML files in the form, gives the
// webview (crates/xml-form-editor-ui) the document, the files it links to and the schemas they
// name, and applies the edits the webview sends back. Everything else, from reading the schemas to
// computing the edits, is Rust.

import * as vscode from "vscode";

const VIEW_TYPE = "xmlFormEditor.form";

/** The forms that are open. */
const sessions = new Set<FormSession>();

/** The changes the forms hold for linked files, until they're saved (see `Held`). */
let held: Held;

export function activate(context: vscode.ExtensionContext): void {
  held = new Held(context.workspaceState);
  // Changes restored from the backup for files that VS Code reopened in tabs
  for (const tab of vscode.window.tabGroups.all.flatMap((group) => group.tabs)) {
    const uri = tabUri(tab);
    if (uri && held.get(uri)) {
      void release(uri);
    }
  }
  context.subscriptions.push(
    held,
    // A file opened in a tab takes the changes the forms hold for it, as unsaved changes
    vscode.window.tabGroups.onDidChangeTabs(({ opened, changed }) => {
      for (const tab of [...opened, ...changed]) {
        const uri = tabUri(tab);
        if (uri && held.get(uri)) {
          void release(uri);
        }
      }
    }),
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
  const tab = vscode.window.tabGroups.activeTabGroup.activeTab;
  return (tab && tabUri(tab)) ?? vscode.window.activeTextEditor?.document.uri;
}

/** The file a tab shows, as text or in a custom editor such as the form. */
function tabUri(tab: vscode.Tab): vscode.Uri | undefined {
  const input = tab.input;
  if (input instanceof vscode.TabInputText || input instanceof vscode.TabInputCustom) {
    return input.uri;
  }
  return undefined;
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
  | { type: "openForm"; document: string }
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
      held.onDidChange(async (uri) => {
        if (this.files.has(uri.toString())) {
          this.sendDocument(await vscode.workspace.openTextDocument(uri));
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
    // Linked files that no other form shows, whose changes the forms may hold
    const linked = [...this.files.values()]
      .map(({ uri }) => uri)
      .filter((uri) => !sameUri(uri, this.document.uri) && ![...sessions].some((s) => s.shows(uri)));
    this.files.forEach((file) => file.watcher?.dispose());
    this.schemas.forEach((schema) => schema.watcher.dispose());
    this.disposables.forEach((d) => d.dispose());
    if (unsaved.length > 0) {
      void offerToSave(unsaved);
    }
    void askToSaveHeld(linked);
  }

  /** Whether the form shows a file. */
  shows(uri: vscode.Uri): boolean {
    return this.files.has(uri.toString());
  }

  /** Saves the file and the files it links to, those with unsaved changes. */
  async saveAll(): Promise<void> {
    for (const { uri } of this.files.values()) {
      if (held.get(uri)) {
        await saveHeld(uri);
        continue;
      }
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

  /** Sends the form a file: the document, or the changes held for it, which aren't saved. */
  private sendDocument(document: vscode.TextDocument): void {
    const uri = document.uri.toString();
    const change = held.get(document.uri);
    const text = change?.text ?? document.getText();
    const file = this.files.get(uri);
    if (file) {
      file.sent = text;
    }
    this.post({
      type: "document",
      uri,
      main: sameUri(document.uri, this.document.uri),
      text,
      version: change?.version ?? document.version,
      dirty: change ? true : document.isDirty,
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
      case "openForm":
        await this.openInForm(message.document);
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
    if (held.takeRestored(document.uri)) {
      void vscode.window.showInformationMessage(
        `The form has the changes to ${basename(document.uri)} that weren't saved when VS Code closed. Save to keep them.`,
      );
    }
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
      const uri = shown.uri;
      // A linked file that no tab shows has its changes held, as VS Code would open a tab for it
      // with unsaved changes; one that a tab shows has them in its document, as unsaved changes
      const hold = !sameUri(uri, this.document.uri) && uri.scheme !== "untitled" && !isInTab(uri);
      if (!hold) {
        await release(uri);
      }
      document = await vscode.workspace.openTextDocument(uri);
      const change = held.get(uri);
      const current = change ?? { text: document.getText(), version: document.version };
      // Edits made against another version would land in the wrong place. Refused, the form makes
      // them again against the new text. The text is compared too, as a file that VS Code closed
      // and opened again counts its versions afresh.
      if (version === current.version && current.text === shown.sent) {
        if (hold) {
          const text = applyTextEdits(current.text, edits);
          const base = change?.base ?? document.getText();
          // Edited back as it was, there's nothing to hold
          held.set(uri, text === base ? undefined : { text, version: current.version + 1, base });
          applied = true;
        } else {
          const edit = new vscode.WorkspaceEdit();
          for (const { start, end, text } of edits) {
            const range = new vscode.Range(document.positionAt(start), document.positionAt(end));
            edit.replace(uri, range, text);
          }
          applied = await vscode.workspace.applyEdit(edit);
          if (!applied) {
            error = "VS Code refused the edit. Is the file read-only?";
          } else if (!sameUri(uri, this.document.uri)) {
            this.changed.add(file);
          }
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
    // The text shown has the changes the form holds, which the offset counts
    await release(shown.uri);
    const document = await vscode.workspace.openTextDocument(shown.uri);
    const editor = await this.showText(document);
    const position = document.positionAt(offset);
    editor.selection = new vscode.Selection(position, position);
    editor.revealRange(new vscode.Range(position, position), vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  }

  /** Opens a file the form shows in a form of its own: the one already showing it, or a new one beside. */
  private async openInForm(file: string): Promise<void> {
    const shown = this.files.get(file);
    if (!shown) {
      return;
    }
    const group = vscode.window.tabGroups.all.find((g) =>
      g.tabs.some(
        (tab) => tab.input instanceof vscode.TabInputCustom && tab.input.viewType === VIEW_TYPE && sameUri(tab.input.uri, shown.uri),
      ),
    );
    // Opening the tab moves the changes the forms hold for the file into its document
    await vscode.commands.executeCommand("vscode.openWith", shown.uri, VIEW_TYPE, group?.viewColumn ?? vscode.ViewColumn.Beside);
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

/** A linked file's text with the changes the forms hold for it (see `Held`). */
type HeldChange = {
  text: string;
  /** Counts the changes, as a document's version does, for the forms to make edits against */
  version: number;
  /** The file's text the changes were made to, to tell whether it has changed since */
  base: string;
};

/** Where `Held` backs up the changes, in the workspace's storage. */
const BACKUP_KEY = "heldChanges";

/**
 * The changes the forms make to linked files that no tab shows, until they're saved. VS Code shows
 * every file with unsaved changes in a tab, so that none go unnoticed, and would open one for each
 * linked file the form changed, so the changes are held here instead of in the files' documents.
 * They're backed up in the workspace's storage, in case VS Code closes before they're saved.
 */
class Held implements vscode.Disposable {
  private readonly changes = new Map<string, HeldChange>();
  /** Files whose changes came from the backup, until a form shows them */
  private readonly restored = new Set<string>();
  private readonly changed = new vscode.EventEmitter<vscode.Uri>();
  /** A file's held changes changed, or were saved or let go */
  readonly onDidChange = this.changed.event;

  constructor(private readonly storage: vscode.Memento) {
    const backup = storage.get<Record<string, { text: string; base: string }>>(BACKUP_KEY) ?? {};
    for (const [uri, { text, base }] of Object.entries(backup)) {
      this.changes.set(uri, { text, base, version: 1 });
      this.restored.add(uri);
    }
  }

  dispose(): void {
    this.changed.dispose();
  }

  get(uri: vscode.Uri): HeldChange | undefined {
    return this.changes.get(uri.toString());
  }

  /** Holds a file's changes, or with `undefined`, lets them go. */
  set(uri: vscode.Uri, change: HeldChange | undefined): void {
    const key = uri.toString();
    if (change) {
      this.changes.set(key, change);
    } else {
      this.changes.delete(key);
    }
    this.restored.delete(key);
    const backup = Object.fromEntries([...this.changes].map(([uri, { text, base }]) => [uri, { text, base }]));
    void this.storage.update(BACKUP_KEY, backup);
    this.changed.fire(uri);
  }

  /** Whether the changes held for a file came from the backup, the first time a form shows it. */
  takeRestored(uri: vscode.Uri): boolean {
    return this.restored.delete(uri.toString());
  }
}

/**
 * Moves the changes held for a file into its document, where they're unsaved changes as any
 * other: as a tab shows the file, or before it does.
 */
async function release(uri: vscode.Uri): Promise<void> {
  const change = held.get(uri);
  if (!change) {
    return;
  }
  const document = await vscode.workspace.openTextDocument(uri);
  // The document has them before they're let go, so that the forms go on showing them
  if (document.getText() === change.text || (await vscode.workspace.applyEdit(replaceAll(document, change.text)))) {
    held.set(uri, undefined);
  } else {
    void vscode.window.showErrorMessage(`Could not put the form's changes into ${displayPath(uri)}.`);
  }
}

/**
 * Saves the changes held for a file. When the file has changed since they were made, that asks
 * first whether to write over it.
 */
async function saveHeld(uri: vscode.Uri): Promise<void> {
  const change = held.get(uri);
  if (!change) {
    return;
  }
  const document = await vscode.workspace.openTextDocument(uri);
  if (document.getText() !== change.base) {
    const overwrite = "Overwrite";
    const discard = "Discard the Form's Changes";
    const choice = await vscode.window.showWarningMessage(
      `${basename(uri)} has changed since the form changed it.`,
      { modal: true, detail: "Overwriting replaces the other changes with the form's." },
      overwrite,
      discard,
    );
    if (choice === discard) {
      held.set(uri, undefined);
    }
    if (choice !== overwrite) {
      return;
    }
  }
  // Saved straight away, the document never has unsaved changes long enough for VS Code to open a
  // tab for it
  if (!(await vscode.workspace.applyEdit(replaceAll(document, change.text)))) {
    void vscode.window.showErrorMessage(`Could not save ${displayPath(uri)}: VS Code refused the changes.`);
    return;
  }
  held.set(uri, undefined);
  const saved = await document.save().then(
    (saved) => saved,
    () => false,
  );
  if (!saved) {
    void vscode.window.showErrorMessage(`Could not save ${displayPath(uri)}.`);
  }
}

/** Asks whether to save the changes held for the files of a form that closed. */
async function askToSaveHeld(uris: vscode.Uri[]): Promise<void> {
  const files = uris.filter((uri) => held.get(uri));
  if (files.length === 0) {
    return;
  }
  const names = files.map(basename).join(", ");
  const save = "Save";
  const dontSave = "Don't Save";
  const choice = await vscode.window.showWarningMessage(
    `Do you want to save the changes the form made to ${names}?`,
    { modal: true, detail: "If you don't, they're lost. Cancel keeps them, for when the form shows them again." },
    save,
    dontSave,
  );
  for (const uri of files) {
    if (choice === save) {
      await saveHeld(uri);
    } else if (choice === dontSave) {
      held.set(uri, undefined);
    }
  }
}

/** An edit that replaces all of a document's text. */
function replaceAll(document: vscode.TextDocument, text: string): vscode.WorkspaceEdit {
  const edit = new vscode.WorkspaceEdit();
  edit.replace(document.uri, new vscode.Range(document.positionAt(0), document.positionAt(document.getText().length)), text);
  return edit;
}

/** Makes edits to a text, as VS Code makes them to a document: offsets count UTF-16 units. */
function applyTextEdits(text: string, edits: TextEdit[]): string {
  // From the end, so that the offsets of those still to make hold
  for (const edit of [...edits].sort((a, b) => b.start - a.start)) {
    text = text.slice(0, edit.start) + edit.text + text.slice(edit.end);
  }
  return text;
}

/** Whether a tab shows a file, as text or in a custom editor such as the form. */
function isInTab(uri: vscode.Uri): boolean {
  return vscode.window.tabGroups.all.some((group) =>
    group.tabs.some((tab) => {
      const shown = tabUri(tab);
      return shown !== undefined && sameUri(shown, uri);
    }),
  );
}

/** Offers to save the linked files that a form changed and didn't save, as it closes. */
async function offerToSave(dirty: vscode.TextDocument[]): Promise<void> {
  // Those edited back as they were have nothing to save
  const differ = await Promise.all(dirty.map(differsFromDisk));
  const documents = dirty.filter((_, i) => differ[i]);
  if (documents.length === 0) {
    return;
  }
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

/** Whether a document's text differs from its file's; `true` when the file can't be read. */
async function differsFromDisk(document: vscode.TextDocument): Promise<boolean> {
  try {
    return new TextDecoder().decode(await vscode.workspace.fs.readFile(document.uri)) !== document.getText();
  } catch {
    return true;
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
