"use client";

import { useState, useRef } from "react";
import { useTodos } from "@/hooks/useTodos";

type Filter = "all" | "active" | "completed";

export default function Home() {
  const { todos, filter, addTodo, toggleTodo, deleteTodo, setFilter } =
    useTodos();
  const [inputValue, setInputValue] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (inputValue.trim()) {
      addTodo(inputValue);
      setInputValue("");
      inputRef.current?.focus();
    }
  };

  return (
    <main className="todo-app" data-anvil-state={JSON.stringify({ todos, filter })}>
      <h1>✅ Todo App</h1>

      <form onSubmit={handleSubmit} className="todo-input-row">
        <input
          ref={inputRef}
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          placeholder="タスクを入力..."
          data-anvil-action="input"
          className="todo-input"
        />
        <button
          type="submit"
          data-anvil-action="primary"
          disabled={!inputValue.trim()}
          className="todo-btn todo-btn-primary"
        >
          追加
        </button>
      </form>

      <div className="filter-bar">
        {(["all", "active", "completed"] as Filter[]).map((f) => (
          <button
            key={f}
            onClick={() => setFilter(f)}
            data-anvil-action={`filter-${f}`}
            className={`todo-btn todo-btn-filter ${filter === f ? "active" : ""}`}
          >
            {f === "all" ? "すべて" : f === "active" ? "未完了" : "完了"}
          </button>
        ))}
      </div>

      <ul className="todo-list">
        {todos.length === 0 && (
          <li className="todo-empty">
            {filter === "all"
              ? "タスクがありません。追加してください。"
              : "該当するタスクはありません。"}
          </li>
        )}
        {todos.map((todo) => (
          <li
            key={todo.id}
            className={`todo-item ${todo.completed ? "completed" : ""}`}
          >
            <input
              type="checkbox"
              checked={todo.completed}
              onChange={() => toggleTodo(todo.id)}
              className="todo-checkbox"
            />
            <span className="todo-text">{todo.text}</span>
            <button
              onClick={() => deleteTodo(todo.id)}
              data-anvil-action={`delete-${todo.id}`}
              className="todo-btn todo-btn-delete"
              title="削除"
            >
              ×
            </button>
          </li>
        ))}
      </ul>

      <div className="todo-stats">
        未完了: {todos.filter((t) => !t.completed).length} | 完了:{" "}
        {todos.filter((t) => t.completed).length} | 合計: {todos.length}
      </div>

      <p className="todo-hint">Enter キーで追加 ・ クリックで完了/未完了切替 ・ ホバーで削除</p>
    </main>
  );
}
