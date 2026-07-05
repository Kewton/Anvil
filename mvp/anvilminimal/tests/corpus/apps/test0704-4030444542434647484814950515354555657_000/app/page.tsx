'use client';

import { useState, useEffect } from 'react';

interface Memo {
  id: string;
  text: string;
}

export default function MemoApp() {
  const [memos, setMemos] = useState<Memo[]>([]);
  const [input, setInput] = useState('');

  useEffect(() => {
    fetch('/api/memos')
      .then((res) => res.json())
      .then((data) => setMemos(data));
  }, []);

  const addMemo = async () => {
    if (!input.trim()) return;
    const res = await fetch('/api/memos', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: input }),
    });
    const newMemo = await res.json();
    setMemos([...memos, newMemo]);
    setInput('');
  };

  const deleteMemo = async (id: string) => {
    await fetch('/api/memos', {
      method: 'DELETE',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id }),
    });
    setMemos(memos.filter((m) => m.id !== id));
  };

  return (
    <main className="max-w-md mx-auto p-4 mt-10">
      <h1 className="text-2xl font-bold mb-4">メモアプリ</h1>
      <div className="flex gap-2 mb-4">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          data-anvil-action="input"
          className="border p-2 flex-grow rounded"
          placeholder="新しいメモを入力..."
        />
        <button
          onClick={addMemo}
          data-anvil-action="primary"
          className="bg-blue-500 text-white p-2 rounded"
        >
          追加
        </button>
      </div>
      <ul className="space-y-2" data-anvil-state={JSON.stringify(memos)}>
        {memos.map((memo) => (
          <li key={memo.id} className="border p-2 rounded bg-gray-50 flex justify-between items-center">
            {memo.text}
            <button onClick={() => deleteMemo(memo.id)} className="text-red-500 text-sm">削除</button>
          </li>
        ))}
      </ul>
    </main>
  );
}
