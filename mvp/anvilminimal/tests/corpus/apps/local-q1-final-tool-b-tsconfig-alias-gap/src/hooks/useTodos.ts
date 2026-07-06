"use client";

import { useState, useEffect, useCallback } from "react";

interface Todo {
  id: number;
  text: string;
  completed: boolean;
}

type Filter = "all" | "active" | "completed";

const STORAGE_KEY = "todo-app-todos";
const FILTER_KEY = "todo-app-filter";

function loadFromStorage<T>(key: string, fallback: T): T {
  if (typeof window === "undefined") return fallback;
  try {
    const raw = localStorage.getItem(key);
    if (raw !== null) return JSON.parse(raw) as T;
  } catch {
    // ignore parse errors
  }
  return fallback;
}

function saveToStorage<T>(key: string, value: T): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // ignore storage errors
  }
}

export function useTodos() {
  const [todos, setTodos] = useState<Todo[]>(() => loadFromStorage(STORAGE_KEY, []));
  const [filter, setFilterState] = useState<Filter>(() => loadFromStorage(FILTER_KEY, "all"));

  // Persist todos on every change
  useEffect(() => {
    saveToStorage(STORAGE_KEY, todos);
  }, [todos]);

  // Persist filter on every change
  useEffect(() => {
    saveToStorage(FILTER_KEY, filter);
  }, [filter]);

  const addTodo = useCallback((text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    setTodos((prev) => [
      ...prev,
      { id: Date.now(), text: trimmed, completed: false },
    ]);
  }, []);

  const toggleTodo = useCallback((id: number) => {
    setTodos((prev) =>
      prev.map((todo) =>
        todo.id === id ? { ...todo, completed: !todo.completed } : todo
      )
    );
  }, []);

  const deleteTodo = useCallback((id: number) => {
    setTodos((prev) => prev.filter((todo) => todo.id !== id));
  }, []);

  const setFilter = useCallback((newFilter: Filter) => {
    setFilterState(newFilter);
  }, []);

  const filteredTodos = todos.filter((todo) => {
    if (filter === "active") return !todo.completed;
    if (filter === "completed") return todo.completed;
    return true;
  });

  return {
    todos: filteredTodos,
    allTodos: todos,
    filter,
    addTodo,
    toggleTodo,
    deleteTodo,
    setFilter,
  };
}
