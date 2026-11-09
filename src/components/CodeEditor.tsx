import { useEffect, useRef } from "react";
import { basicSetup } from "codemirror";
import { Compartment, EditorState, Prec, Transaction } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { indentWithTab, redo } from "@codemirror/commands";
import { closeCompletion } from "@codemirror/autocomplete";
import { closeSearchPanel } from "@codemirror/search";
import {
  HighlightStyle,
  LanguageDescription,
  syntaxHighlighting,
} from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { tags } from "@lezer/highlight";

const colors = HighlightStyle.define([
  {
    tag: [tags.keyword, tags.modifier, tags.operatorKeyword],
    color: "var(--code-keyword)",
  },
  { tag: [tags.string, tags.regexp], color: "var(--code-string)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--code-number)" },
  {
    tag: [tags.comment, tags.meta],
    color: "var(--text-3)",
    fontStyle: "italic",
  },
  {
    tag: [tags.function(tags.variableName), tags.definition(tags.variableName)],
    color: "var(--code-function)",
  },
  {
    tag: [tags.typeName, tags.className, tags.tagName],
    color: "var(--code-type)",
  },
  {
    tag: [tags.propertyName, tags.attributeName],
    color: "var(--code-property)",
  },
]);

function languageFor(filename: string) {
  const name = filename.split("/").pop() || filename;
  const hint = /^\.env(?:\.|$)|^(?:\.bashrc|\.zshrc|\.profile)$/.test(name)
    ? "Shell"
    : /^nginx.*\.conf$/.test(name)
      ? "Nginx"
      : /\.(?:conf|ini|cfg)$/.test(name)
        ? "Properties files"
        : undefined;
  return hint
    ? LanguageDescription.matchLanguageName(languages, hint)
    : LanguageDescription.matchFilename(languages, name);
}

export type EditorStats = { lines: number; line: number; column: number };

export default function CodeEditor(props: {
  filename: string;
  value: string;
  readOnly: boolean;
  wrap: boolean;
  onChange: (value: string) => void;
  onSave: () => void;
  onStats: (stats: EditorStats) => void;
  onLanguage: (language: string) => void;
  onEscapeReady?: (handler: () => boolean) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const compartments = useRef({
    mode: new Compartment(),
    wrap: new Compartment(),
    language: new Compartment(),
  });

  useEffect(() => {
    const { mode, wrap, language } = compartments.current;
    const editor = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: latest.current.value,
        extensions: [
          basicSetup,
          EditorState.phrases.of({
            Find: "查找",
            Replace: "替换为",
            next: "下一处",
            previous: "上一处",
            all: "全部选中",
            "match case": "区分大小写",
            regexp: "正则表达式",
            "by word": "全字匹配",
            replace: "替换",
            "replace all": "全部替换",
            close: "关闭搜索",
          }),
          EditorState.lineSeparator.of(
            latest.current.value.includes("\r\n")
              ? "\r\n"
              : latest.current.value.includes("\r")
                ? "\r"
                : "\n",
          ),
          syntaxHighlighting(colors),
          mode.of([]),
          wrap.of([]),
          language.of([]),
          Prec.highest(
            keymap.of([
              {
                key: "Mod-s",
                preventDefault: true,
                run: () => {
                  latest.current.onSave();
                  return true;
                },
              },
              indentWithTab,
              { key: "Mod-Shift-z", run: redo, preventDefault: true },
            ]),
          ),
          EditorView.updateListener.of((update) => {
            if (update.docChanged)
              latest.current.onChange(update.state.sliceDoc());
            if (update.docChanged || update.selectionSet) {
              const pos = update.state.selection.main.head;
              const line = update.state.doc.lineAt(pos);
              latest.current.onStats({
                lines: update.state.doc.lines,
                line: line.number,
                column: pos - line.from + 1,
              });
            }
          }),
          EditorView.theme({
            "&": {
              height: "100%",
              color: "var(--text-1)",
              backgroundColor: "var(--bg-card)",
            },
            ".cm-scroller": {
              overflow: "auto",
              fontFamily: "var(--font-code)",
              fontSize: "13px",
              lineHeight: "1.7",
            },
            ".cm-content": { padding: "12px 0", caretColor: "var(--text-1)" },
            ".cm-line": { padding: "0 16px" },
            ".cm-gutters": {
              color: "var(--text-3)",
              backgroundColor: "var(--bg-sidebar)",
              borderRight: "1px solid var(--border-default)",
            },
            ".cm-activeLine": {
              backgroundColor: "var(--code-active-line)",
            },
            ".cm-activeLineGutter": {
              backgroundColor: "var(--action-soft)",
            },
            "&.cm-focused": { outline: "none" },
            ".cm-selectionBackground": {
              backgroundColor: "var(--code-selection-inactive)",
            },
            "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":
              {
                backgroundColor: "var(--code-selection)",
              },
            // CodeMirror draws selections behind the syntax-highlighted text.
            // Override the global white selection foreground, including token spans.
            ".cm-content ::selection, .cm-content::selection": {
              color: "currentColor",
              backgroundColor: "transparent",
            },
            ".cm-cursor": { borderLeftColor: "var(--text-1)" },
            ".cm-panels": {
              color: "var(--text-1)",
              backgroundColor: "var(--bg-sidebar)",
            },
            ".cm-panels-bottom": {
              borderTop: "1px solid var(--border-default)",
            },
            ".cm-panel.cm-search": {
              padding: "8px 48px 8px 14px",
              fontFamily: "var(--font-interface)",
              fontSize: "12px",
              lineHeight: "36px",
            },
            ".cm-search input, .cm-search button, .cm-search label": {
              margin: "3px 6px 3px 0",
              verticalAlign: "middle",
            },
            ".cm-search .cm-textfield": {
              width: "clamp(140px, 28vw, 260px)",
              maxWidth: "100%",
              height: "30px",
              padding: "0 10px",
              font: "inherit",
              lineHeight: "normal",
              borderRadius: "6px",
            },
            ".cm-search .cm-textfield::placeholder": {
              color: "var(--text-3)",
            },
            ".cm-search .cm-textfield:focus": {
              borderColor: "var(--action-bg)",
              outline: "1px solid var(--action-bg)",
              outlineOffset: "-1px",
            },
            ".cm-search .cm-button": {
              height: "30px",
              padding: "0 10px",
              font: "inherit",
              lineHeight: "normal",
              borderRadius: "6px",
              cursor: "pointer",
            },
            ".cm-search .cm-button:hover": {
              color: "var(--action-text)",
              background: "var(--action-soft)",
              borderColor: "var(--action-bg)",
            },
            // Keep global form label/input rules out of the search options.
            ".cm-search label": {
              display: "inline-flex",
              flexDirection: "row",
              alignItems: "center",
              gap: "6px",
              marginRight: "12px",
              color: "var(--text-2)",
              fontSize: "12px",
              lineHeight: "30px",
              whiteSpace: "nowrap",
              cursor: "pointer",
            },
            '.cm-search input[type="checkbox"]': {
              appearance: "auto",
              flex: "0 0 14px",
              width: "14px",
              height: "14px",
              minHeight: "0",
              margin: "0",
              padding: "0",
              accentColor: "var(--action-bg)",
              cursor: "pointer",
            },
            '.cm-panel.cm-search button[name="close"]': {
              top: "50%",
              transform: "translateY(-50%)",
              right: "12px",
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              width: "30px",
              height: "30px",
              padding: "0",
              margin: "0",
              borderRadius: "6px",
              color: "var(--text-3)",
              fontSize: "20px",
              lineHeight: "1",
              cursor: "pointer",
            },
            '.cm-panel.cm-search button[name="close"]:hover': {
              color: "var(--text-1)",
              background: "var(--bg-selected)",
            },
            '.cm-search button:focus-visible, .cm-search input[type="checkbox"]:focus-visible':
              {
                outline: "2px solid var(--action-bg)",
                outlineOffset: "2px",
              },
            ".cm-textfield, .cm-button": {
              color: "var(--text-1)",
              background: "var(--bg-card)",
              border: "1px solid var(--border-strong)",
              borderRadius: "4px",
            },
            ".cm-tooltip": {
              color: "var(--text-1)",
              backgroundColor: "var(--bg-card)",
              borderColor: "var(--border-strong)",
            },
          }),
        ],
      }),
    });
    view.current = editor;
    latest.current.onEscapeReady?.(
      () => closeCompletion(editor) || closeSearchPanel(editor),
    );
    latest.current.onStats({
      lines: editor.state.doc.lines,
      line: 1,
      column: 1,
    });
    const description = languageFor(props.filename);
    latest.current.onLanguage(description?.name || "纯文本");
    let active = true;
    void description
      ?.load()
      .then((support) => {
        if (active) editor.dispatch({ effects: language.reconfigure(support) });
      })
      .catch(() => {
        if (active) latest.current.onLanguage("纯文本");
      });
    return () => {
      active = false;
      latest.current.onEscapeReady?.(() => false);
      view.current = null;
      editor.destroy();
    };
  }, [props.filename]);

  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    editor.dispatch({
      effects: compartments.current.mode.reconfigure([
        EditorState.readOnly.of(props.readOnly),
        EditorView.contentAttributes.of({
          "aria-label": props.readOnly ? "文件预览" : "文件内容",
          "aria-readonly": String(props.readOnly),
          spellcheck: "false",
        }),
      ]),
    });
    if (!props.readOnly) editor.focus();
  }, [props.readOnly, props.filename]);

  useEffect(() => {
    view.current?.dispatch({
      effects: compartments.current.wrap.reconfigure(
        props.wrap ? EditorView.lineWrapping : [],
      ),
    });
  }, [props.wrap, props.filename]);

  useEffect(() => {
    const editor = view.current;
    if (editor && editor.state.sliceDoc() !== props.value) {
      // External loads must not create an undo step restoring a truncated preview.
      editor.dispatch({
        changes: { from: 0, to: editor.state.doc.length, insert: props.value },
        annotations: Transaction.addToHistory.of(false),
      });
    }
  }, [props.value]);

  return <div ref={host} className="remote-code-editor" />;
}
