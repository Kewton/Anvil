"use client";

import { useState, useEffect } from "react";
import {
  loadTodos,
  saveTodos,
  addTodo as storageAddTodo,
  toggleTodo as storageToggleTodo,
  deleteTodo as storageDeleteTodo,
  clearAll as storageClearAll,
  filterTodos,
  type Todo,
  type Filter,
} from "@/lib/todo-storage";

export default function Home() {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [input, setInput] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [ready, setReady] = useState(false);

  // Load from localStorage on mount
  useEffect(() => {
    const stored = loadTodos();
    setTodos(stored);
    setReady(true);
  }, []);

  // Persist to localStorage whenever todos change
  useEffect(() => {
    if (!ready) return;
    saveTodos(todos);
  }, [todos, ready]);

  const handleAdd = () => {
    const trimmed = input.trim();
    if (!trimmed) return;
    const created = storageAddTodo(trimmed);
    if (created) {
      setTodos((prev) => [...prev, created]);
      setInput("");
    }
  };

  const handleToggle = (id: number) => {
    storageToggleTodo(id);
    setTodos((prev) =>
      prev.map((t) => (t.id === id ? { ...t, completed: !t.completed } : t))
    );
  };

  const handleDelete = (id: number) => {
    storageDeleteTodo(id);
    setTodos((prev) => prev.filter((t) => t.id !== id));
  };

  const handleClearAll = () => {
    storageClearAll();
    setTodos([]);
  };

  const filtered = filterTodos(todos, filter);
  const stateSnapshot = JSON.stringify(
    { todos: filtered, total: todos.length, active: todos.filter((t) => !t.completed).length, completed: todos.filter((t) => t.completed).length, filter },
    null,
    2
  );

  return (
    <main className="min-h-screen bg-gray-50 text-gray-900 flex items-start justify-center pt-16 px-4">
      <div className="w-full max-w-lg">
        <h1 className="text-3xl font-bold mb-6 text-center">Todo List</h1>

        {/* Input area */}
        <div className="flex gap-2 mb-6">
          <input
            data-anvil-action="input"
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleAdd()}
            placeholder="What needs to be done?"
            className="flex-1 px-4 py-2 rounded-lg border border-gray-300 focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
          <button
            data-anvil-action="primary"
            onClick={handleAdd}
            className="px-5 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 font-medium transition-colors"
          >
            Add
          </button>
        </div>

        {/* Filter tabs */}
        <div className="flex gap-1 mb-4 border-b border-gray-200">
          {(["all", "active", "completed"] as Filter[]).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`px-3 py-1.5 text-sm font-medium rounded-t transition-colors ${
                filter === f
                  ? "bg-blue-600 text-white"
                  : "text-gray-600 hover:bg-gray-200"
              }`}
            >
              {f.charAt(0).toUpperCase() + f.slice(1)}
            </button>
          ))}
        </div>

        {/* Todo list */}
        <ul className="space-y-2 mb-6">
          {filtered.map((todo) => (
            <li
              key={todo.id}
              className="flex items-center gap-3 bg-white rounded-lg border border-gray-200 px-4 py-3 shadow-sm"
            >
              <input
                type="checkbox"
                checked={todo.completed}
                onChange={() => handleToggle(todo.id)}
                className="w-5 h-5 accent-blue-600 cursor-pointer"
              />
              <span
                className={`flex-1 text-base ${
                  todo.completed ? "line-through text-gray-400" : ""
                }`}
              >
                {todo.text}
              </span>
              <button
                onClick={() => handleDelete(todo.id)}
                className="text-red-500 hover:text-red-700 font-bold text-lg leading-none transition-colors"
                title="Delete"
              >
                ×
              </button>
            </li>
          ))}
        </ul>

        {filtered.length === 0 && (
          <p className="text-center text-gray-400 mb-6">No todos here.</p>
        )}

        {/* Clear all / restart */}
        <div className="flex justify-center gap-3">
          <button
            data-anvil-action="restart"
            onClick={handleClearAll}
            className="px-5 py-2 bg-gray-200 text-gray-700 rounded-lg hover:bg-red-100 hover:text-red-600 font-medium transition-colors"
          >
            Clear All
          </button>
        </div>

        {/* Hidden state snapshot for observability */}
        <pre
          data-anvil-state={stateSnapshot}
          className="hidden"
          aria-hidden="true"
        />
      </div>
    </main>
  );
}
