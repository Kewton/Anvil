"use client";

import { useState, useEffect, useCallback } from "react";

interface Note {
  id: string;
  title: string;
  content: string;
  updatedAt: string;
}

const STORAGE_KEY = "markdown-notes-app-data";

function generateId(): string {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

function loadNotes(): Note[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return JSON.parse(raw);
  } catch {
    /* ignore */
  }
  return [];
}

function saveNotes(notes: Note[]): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(notes));
  } catch {
    /* ignore */
  }
}

/**
 * Minimal Markdown-to-HTML renderer.
 * Supports headings (# ## ### #### ##### ######), bold (**text**), italic (*text*),
 * inline code (`code`), fenced code blocks (``` ... ```), unordered lists (- item),
 * and horizontal rules (---).
 */
function markdownToHtml(md: string): string {
  let html = md;

  // Fenced code blocks first to avoid inner processing
  html = html.replace(/```([\s\S]*?)```/g, (_, code) => {
    return `<pre><code>${escapeHtml(code.trim())}</code></pre>`;
  });

  const lines = html.split("\n");
  const result: string[] = [];
  let inList = false;

  for (const line of lines) {
    // Horizontal rule
    if (/^---+$/.test(line.trim())) {
      if (inList) { result.push("</ul>"); inList = false; }
      result.push("<hr />");
      continue;
    }

    // Unordered list items
    const listMatch = line.match(/^[\t ]*- (.+)$/);
    if (listMatch) {
      if (!inList) { result.push("<ul>"); inList = true; }
      result.push(`<li>${inlineFormat(escapeHtml(listMatch[1]))}</li>`);
      continue;
    }

    // Close list if we're no longer in one
    if (inList && line.trim() === "") {
      result.push("</ul>");
      inList = false;
    } else if (inList) {
      result.push("</ul>");
      inList = false;
    }

    // Headings
    const headingMatch = line.match(/^(#{1,6})\s+(.+)$/);
    if (headingMatch) {
      const level = headingMatch[1].length;
      result.push(`<h${level}>${inlineFormat(escapeHtml(headingMatch[2]))}</h${level}>`);
      continue;
    }

    // Empty line
    if (line.trim() === "") {
      result.push("");
      continue;
    }

    result.push(`<p>${inlineFormat(escapeHtml(line))}</p>`);
  }

  if (inList) result.push("</ul>");

  return result.join("\n");
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function inlineFormat(text: string): string {
  // Bold **text** or __text__
  text = text.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  text = text.replace(/__(.+?)__/g, "<strong>$1</strong>");
  // Italic *text* or _text_
  text = text.replace(/\*(.+?)\*/g, "<em>$1</em>");
  text = text.replace(/_(.+?)_/g, "<em>$1</em>");
  // Inline code `code`
  text = text.replace(/`(.+?)`/g, "<code class='inline-code'>$1</code>");
  return text;
}

export default function Home() {
  const [notes, setNotes] = useState<Note[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [searchQuery, setSearchQuery] = useState("");

  // Load notes from localStorage on mount
  useEffect(() => {
    setNotes(loadNotes());
  }, []);

  // Persist notes whenever they change
  useEffect(() => {
    saveNotes(notes);
  }, [notes]);

  const activeNote = notes.find((n) => n.id === activeId) ?? null;

  const handleCreate = useCallback(() => {
    const newNote: Note = {
      id: generateId(),
      title: "Untitled",
      content: "",
      updatedAt: new Date().toISOString(),
    };
    setNotes((prev) => [newNote, ...prev]);
    setActiveId(newNote.id);
    setTitle("Untitled");
    setContent("");
  }, []);

  const handleSave = useCallback(() => {
    if (!activeId) return;
    setNotes((prev) =>
      prev.map((n) =>
        n.id === activeId
          ? { ...n, title, content, updatedAt: new Date().toISOString() }
          : n
      )
    );
  }, [activeId, title, content]);

  const handleDelete = useCallback(
    (id: string) => {
      setNotes((prev) => prev.filter((n) => n.id !== id));
      if (activeId === id) {
        setActiveId(null);
        setTitle("");
        setContent("");
      }
    },
    [activeId]
  );

  const handleSelectNote = useCallback(
    (note: Note) => {
      // Auto-save current note before switching
      if (activeId && title) {
        setNotes((prev) =>
          prev.map((n) =>
            n.id === activeId
              ? { ...n, title, content, updatedAt: new Date().toISOString() }
              : n
          )
        );
      }
      setActiveId(note.id);
      setTitle(note.title);
      setContent(note.content);
    },
    [activeId, title, content]
  );

  const handleRestart = useCallback(() => {
    setNotes([]);
    setActiveId(null);
    setTitle("");
    setContent("");
    setSearchQuery("");
    if (typeof window !== "undefined") {
      localStorage.removeItem(STORAGE_KEY);
    }
  }, []);

  const filteredNotes = notes.filter((n) =>
    n.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
    n.content.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const stateSnapshot = {
    noteCount: notes.length,
    activeNoteId: activeId ?? null,
    activeTitle: title,
    contentLength: content.length,
  };

  return (
    <main className="min-h-screen bg-gray-50 flex flex-col" data-anvil-state={JSON.stringify(stateSnapshot)}>
      {/* Header */}
      <header className="bg-white border-b px-6 py-4 flex items-center justify-between">
        <h1 className="text-xl font-bold text-gray-900">Markdown Note App</h1>
        <div className="flex gap-2">
          <button
            data-anvil-action="primary"
            onClick={handleCreate}
            className="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700 transition-colors font-medium"
          >
            + New Note
          </button>
          <button
            data-anvil-action="restart"
            onClick={handleRestart}
            className="px-4 py-2 bg-gray-200 text-gray-700 rounded hover:bg-gray-300 transition-colors font-medium"
          >
            Clear All
          </button>
        </div>
      </header>

      <div className="flex flex-1 overflow-hidden">
        {/* Sidebar: Note List */}
        <aside className="w-72 bg-white border-r flex flex-col overflow-hidden">
          <div className="p-3 border-b">
            <input
              data-anvil-action="search"
              type="text"
              placeholder="Search notes..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="w-full px-3 py-2 border rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-400"
            />
          </div>
          <div className="flex-1 overflow-y-auto">
            {filteredNotes.length === 0 ? (
              <p className="p-4 text-gray-500 text-sm">No notes yet.</p>
            ) : (
              filteredNotes.map((note) => (
                <div
                  key={note.id}
                  onClick={() => handleSelectNote(note)}
                  className={`flex items-center justify-between px-4 py-3 cursor-pointer border-b hover:bg-blue-50 transition-colors ${
                    activeId === note.id ? "bg-blue-100" : ""
                  }`}
                >
                  <div className="min-w-0 flex-1">
                    <p className="font-medium text-gray-900 truncate">{note.title || "Untitled"}</p>
                    <p className="text-xs text-gray-500 truncate">
                      {new Date(note.updatedAt).toLocaleDateString()}
                    </p>
                  </div>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDelete(note.id);
                    }}
                    className="ml-2 text-red-400 hover:text-red-600 flex-shrink-0"
                    title="Delete note"
                  >
                    ✕
                  </button>
                </div>
              ))
            )}
          </div>
        </aside>

        {/* Main: Editor + Preview */}
        <section className="flex-1 flex overflow-hidden">
          {activeId ? (
            <>
              {/* Editor Pane */}
              <div className="flex-1 flex flex-col border-r">
                <input
                  data-anvil-action="title-input"
                  type="text"
                  placeholder="Note title..."
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  className="w-full px-6 py-3 text-lg font-semibold border-b focus:outline-none bg-white"
                />
                <textarea
                  data-anvil-action="input"
                  placeholder="Write your Markdown here..."
                  value={content}
                  onChange={(e) => setContent(e.target.value)}
                  className="flex-1 w-full p-6 font-mono text-sm resize-none focus:outline-none bg-white leading-relaxed"
                  spellCheck={false}
                />
              </div>

              {/* Preview Pane */}
              <div className="w-1/2 flex flex-col overflow-hidden">
                <div className="px-6 py-3 border-b bg-gray-50 font-medium text-sm text-gray-700">Preview</div>
                <div
                  className="flex-1 p-6 overflow-y-auto prose max-w-none"
                  dangerouslySetInnerHTML={{ __html: markdownToHtml(content) }}
                />
              </div>
            </>
          ) : (
            <div className="flex-1 flex items-center justify-center text-gray-400">
              <p>Select a note or create a new one to get started.</p>
            </div>
          )}
        </section>
      </div>

      {/* Footer status bar */}
      <footer className="bg-white border-t px-6 py-2 text-xs text-gray-500 flex justify-between">
        <span>{notes.length} note{notes.length !== 1 ? "s" : ""}</span>
        {activeNote && (
          <span>Last saved: {new Date(activeNote.updatedAt).toLocaleString()}</span>
        )}
      </footer>

      {/* Global styles for preview content */}
      <style jsx global>{`
        .prose h1, .prose h2, .prose h3, .prose h4, .prose h5, .prose h6 {
          margin-top: 1em;
          margin-bottom: 0.5em;
          font-weight: bold;
        }
        .prose h1 { font-size: 2em; }
        .prose h2 { font-size: 1.5em; }
        .prose h3 { font-size: 1.25em; }
        .prose p { margin-bottom: 0.75em; line-height: 1.7; }
        .prose ul { list-style-type: disc; padding-left: 2em; margin-bottom: 0.75em; }
        .prose li { margin-bottom: 0.25em; }
        .prose pre { background: #f3f4f6; padding: 1em; border-radius: 0.5em; overflow-x: auto; margin-bottom: 1em; }
        .prose code, .prose .inline-code {
          background: #f3f4f6;
          padding: 0.125em 0.375em;
          border-radius: 0.25em;
          font-size: 0.9em;
        }
        .prose pre code {
          background: transparent;
          padding: 0;
        }
        .prose hr {
          border: none;
          border-top: 1px solid #e5e7eb;
          margin: 1.5em 0;
        }
        textarea[data-anvil-action="input"]::placeholder {
          color: #9ca3af;
        }
      `}</style>
    </main>
  );
}
