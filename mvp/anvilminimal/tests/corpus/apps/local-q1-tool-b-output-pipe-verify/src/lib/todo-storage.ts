const STORAGE_KEY = "todo-app-items";

export interface Todo {
  id: number;
  text: string;
  completed: boolean;
}

export type Filter = "all" | "active" | "completed";

/** Load todos from localStorage. Returns [] on missing/corrupt data. */
export function loadTodos(): Todo[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/** Save todos to localStorage. */
export function saveTodos(todos: Todo[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(todos));
  } catch {
    // quota exceeded or unavailable — ignore
  }
}

/** Add a new todo with the given text. Returns the created todo. */
export function addTodo(text: string): Todo | null {
  const trimmed = String(text || "").trim();
  if (!trimmed) return null;
  const todo: Todo = { id: Date.now(), text: trimmed, completed: false };
  const existing = loadTodos();
  saveTodos([...existing, todo]);
  return todo;
}

/** Toggle the completed state of a todo by id. */
export function toggleTodo(id: number): void {
  const todos = loadTodos();
  const updated = todos.map((t) =>
    t.id === id ? { ...t, completed: !t.completed } : t
  );
  saveTodos(updated);
}

/** Delete a todo by id. */
export function deleteTodo(id: number): void {
  const todos = loadTodos();
  saveTodos(todos.filter((t) => t.id !== id));
}

/** Clear all todos from storage. */
export function clearAll(): void {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}

/** Filter a list of todos based on the given filter type. */
export function filterTodos(todos: Todo[], filter: Filter): Todo[] {
  switch (filter) {
    case "active":
      return todos.filter((t) => !t.completed);
    case "completed":
      return todos.filter((t) => t.completed);
    default:
      return todos;
  }
}
