import { useEffect, useRef } from "react";
import {
  createJSONEditor,
  Mode,
  type JsonEditor,
  type Content,
} from "vanilla-jsoneditor";

interface JsonEditorProps {
  content: Content;
}

export function JsonEditorView({ content }: JsonEditorProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<JsonEditor | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    const editor = createJSONEditor({
      target: containerRef.current,
      props: {
        content,
        readOnly: true,
        mainMenuBar: false,
        navigationBar: false,
        mode: Mode.tree,
      },
    });
    editorRef.current = editor;

    return () => {
      editor.destroy();
      editorRef.current = null;
    };
    // Only create/destroy once per mount. Content updates handled below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (editorRef.current) {
      void editorRef.current.set(content);
    }
  }, [content]);

  return (
    <div
      ref={containerRef}
      className="json-editor-dark jse-theme-dark"
      onClick={(e) => e.stopPropagation()}
    />
  );
}
