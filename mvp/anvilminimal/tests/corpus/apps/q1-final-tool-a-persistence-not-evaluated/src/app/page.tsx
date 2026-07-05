"use client";

import { useState, useEffect } from "react";

interface Todo {
  id: string;
  text: string;
  completed: boolean;
}

type Filter = "all" | "active" | "completed";

export default function TodoPage() {
  const [todos, setTodos] = useState<Todo[]>([]);
  const [filter, setFilter] = useState<Filter>("all");
  const [inputValue, setInputValue] = useState("");
  const [mounted, setMounted] = useState(false);

  // Load from localStorage on mount
  useEffect(() => {
    const saved = localStorage.getItem("anvil_todos");
    if (saved) {
      try {
        setTodos(JSON.parse(saved));
      } catch (e) {
        console.error("Failed to parse todos", e);
      }
    }
    const savedFilter = localStorage.getItem("anvil_filter") as Filter;
    if (savedFilter && ["all", "active", "completed"].includes(savedFilter)) {
      setFilter(savedFilter);
    }
    setMounted(true);
  }, []);

  // Save to localStorage when todos or filter change
  useEffect(() => {
    if (!mounted) return;
    localStorage.setItem("anvil_todos", JSON.stringify(todos));
    localStorage.setItem("anvil_filter", filter);
  }, [todos, filter, mounted]);

  const addTodo = (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    if (!inputValue.trim()) return;
    const newTodo: Todo = {
      id: crypto.randomUUID ? crypto.randomUUID() : Math.random().toString(36).substring(2, 9),
      text: inputValue.trim(),
      completed: false,
    };
    setTodos((prev) => [...prev, newTodo]);
    setInputValue("");
  };

  const toggleTodo = (id: string) => {
    setTodos((prev) =>
      prev.map((todo) =>
        todo.id === id ? { ...todo, completed: !todo.completed } : todo
      )
    );
  };

  const deleteTodo = (id: string) => {
    setTodos((prev) => prev.filter((todo) => todo.id !== id));
  };

  const resetAll = () => {
    setTodos([]);
    setFilter("all");
    setInputValue("");
    localStorage.removeItem("anvil_todos");
    localStorage.removeItem("anvil_filter");
  };

  const filteredTodos = todos.filter((todo) => {
    if (filter === "active") return !todo.completed;
    if (filter === "completed") return todo.completed;
    return true;
  });

  const stateJson = JSON.stringify({
    todos,
    filter,
    inputValue,
    mounted,
  });

  return (
    <div
      className="min-h-screen bg-slate-50 text-slate-800 p-4 sm:p-8 font-sans"
      data-anvil-state={stateJson}
    >
      <div className="max-w-md mx-auto bg-white rounded-xl shadow-md p-6 mt-10 border border-slate-100">
        <div className="flex justify-between items-center mb-6">
          <h1 className="text-2xl font-bold text-indigo-600">Todo List</h1>
          <button
            onClick={resetAll}
            data-anvil-action="restart"
            className="text-xs bg-red-100 hover:bg-red-200 text-red-700 px-3 py-1.5 rounded transition duration-150 font-semibold"
            title="Reset storage and all state"
          >
            Reset All
          </button>
        </div>

        <form onSubmit={addTodo} className="mb-6 flex gap-2">
          <input
            type="text"
            value={inputValue}
            onChange={(e) => setInputValue(e.target.value)}
            placeholder="Add a new task..."
            data-anvil-action="input"
            className="flex-grow border border-slate-300 rounded px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-indigo-500"
          />
          <button
            type="submit"
            data-anvil-action="primary"
            className="bg-indigo-600 hover:bg-indigo-700 text-white font-medium text-sm px-4 py-2 rounded transition duration-150"
          >
            Add
          </button>
        </form>

        <div className="flex justify-center border-b border-slate-200 mb-4 pb-2 gap-4">
          <button
            onClick={() => setFilter("all")}
            className={`text-sm pb-1 px-2 font-medium transition duration-150 ${
              filter === "all"
                ? "text-indigo-600 border-b-2 border-indigo-600 font-semibold"
                : "text-slate-500 hover:text-slate-800"
            }`}
          >
            All ({todos.length})
          </button>
          <button
            onClick={() => setFilter("active")}
            className={`text-sm pb-1 px-2 font-medium transition duration-150 ${
              filter === "active"
                ? "text-indigo-600 border-b-2 border-indigo-600 font-semibold"
                : "text-slate-500 hover:text-slate-800"
            }`}
          >
            Active ({todos.filter((t) => !t.completed).length})
          </button>
          <button
            onClick={() => setFilter("completed")}
            className={`text-sm pb-1 px-2 font-medium transition duration-150 ${
              filter === "completed"
                ? "text-indigo-600 border-b-2 border-indigo-600 font-semibold"
                : "text-slate-500 hover:text-slate-800"
            }`}
          >
            Completed ({todos.filter((t) => t.completed).length})
          </button>
        </div>

        {mounted ? (
          filteredTodos.length === 0 ? (
            <div className="text-center py-8 text-slate-400 text-sm">
              No tasks to show.
            </div>
          ) : (
            <ul className="space-y-2">
              {filteredTodos.map((todo) => (
                <li
                  key={todo.id}
                  className="flex items-center justify-between p-3 bg-slate-50 hover:bg-slate-100 rounded border border-slate-200 transition duration-150"
                >
                  <label className="flex items-center gap-3 cursor-pointer flex-grow select-none">
                    <input
                      type="checkbox"
                      checked={todo.completed}
                      onChange={() => toggleTodo(todo.id)}
                      className="w-4 h-4 text-indigo-600 border-slate-300 rounded focus:ring-indigo-500"
                    />
                    <span
                      className={`text-sm transition duration-150 ${
                        todo.completed
                          ? "line-through text-slate-400 font-normal"
                          : "text-slate-700 font-medium"
                      }`}
                    >
                      {todo.text}
                    </span>
                  </label>
                  <button
                    onClick={() => deleteTodo(todo.id)}
                    className="text-red-500 hover:text-red-700 p-1 rounded transition duration-150"
                    aria-label="Delete todo"
                  >
                    <svg
                      xmlns="http://www.w3.org/2000/svg"
                      fill="none"
                      viewBox="0 0 24 24"
                      strokeWidth={1.5}
                      stroke="currentColor"
                      className="w-4 h-4"
                    >
                      <path
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0"
                      />
                    </svg>
                  </button>
                </li>
              ))}
            </ul>
          )
        ) : (
          <div className="text-center py-8 text-slate-400 text-sm">
            Loading...
          </div>
        )}
      </div>
    </div>
  );
}
