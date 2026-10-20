// A Pure plugin in JavaScript: `upper` uppercases the selections.
import { currentDocument, text, selections, applyEdits, setStatus } from 'helix:plugin/host@0.3.0';
import { setStatusline } from 'helix:plugin/decorations@0.3.0';

const noop = () => {};

export const plugin = {
  init(_config) {
    setStatusline('js', 'JS ✓');
    return { commands: [{ name: 'upper', doc: 'Uppercase the selections (JavaScript)' }], events: [] };
  },
  runCommand(name, _args) {
    if (name !== 'upper') throw `unknown command ${name}`;
    const doc = currentDocument();
    if (doc === undefined) throw 'no document';
    const chars = [...text(doc)];
    const edits = selections(doc).map(({ anchor, head }) => {
      const [from, to] = anchor < head ? [anchor, head] : [head, anchor];
      return { from, to, text: chars.slice(from, to).join('').toUpperCase() };
    });
    applyEdits(doc, edits);
    setStatus(`uppercased ${edits.length} selection(s) from JavaScript`);
  },
  onDocumentOpen: noop,
  onDocumentChange: noop,
  onModeSwitch: noop,
};

export const callbacks = {
  onProcessDone: noop, onPickerDone: noop, onPickerQuery: noop, onProcessOutput: noop,
  onPopupClose: noop, onPromptDone: noop, onBackgroundDone: noop, onTimer: noop,
};
export const events = {
  onDocumentSave: noop, onDocumentClose: noop, onSelectionChange: noop, onPostCommand: noop,
  onFocusGained: noop, onFocusLost: noop, onEditorIdle: noop, onCompletion: noop,
  onBufferWrite: noop,
};
export const background = {
  run(task, _input) { throw `no background task ${task}`; },
};
