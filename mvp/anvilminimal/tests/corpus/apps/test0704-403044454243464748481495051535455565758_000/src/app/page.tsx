'use client';

import { useState, useEffect } from 'react';

interface Memo {
  id: string;
  content: string;
  createdAt: number;
}

export default function MemoApp() {
  const [memos, setMemos] = useState<Memo[]>([]);
  const [input, setInput] = useState('');

  const fetchMemos = async () => {
    const res = await fetch('/api/memos');
    const data = await res.json();
    setMemos(data);
  };

  useEffect(() => {
    fetchMemos();
  }, []);

  const addMemo = async () => {
    if (!input.trim()) return;
    await fetch('/api/memos', {
      method: 'POST',
      body: JSON.stringify({ content: input }),
    });
    setInput('');
    fetchMemos();
  };

  const deleteMemo = async (id: string) => {
    await fetch(`/api/memos/${id}`, { method: 'DELETE' });
    fetchMemos();
  };

  const stateJson = JSON.stringify({ memoCount: memos.length, inputLength: input.length });

  return (
    <main 
      className="p-8 max-w-2xl mx-auto" 
      data-anvil-state={stateJson}
    >
      <h1 className="text-2xl font-bold mb-4">メモアプリ</h1>
      <div className="flex gap-2 mb-6">
        <input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          className="border p-2 flex-grow rounded"
          placeholder="メモを入力..."
          data-anvil-action="input"
        />
        <button 
          onClick={addMemo} 
          className="bg-blue-500 text-white px-4 py-2 rounded"
          data-anvil-action="primary"
        >
          追加
        </button>
      </div>
      <ul className="space-y-2">
        {memos.map((memo) => (
          <li key={memo.id} className="border p-3 rounded bg-gray-50 flex justify-between items-center">
            <span>{memo.content}</span>
            <button 
              onClick={() => deleteMemo(memo.id)}
              className="text-red-500 text-sm"
            >
              削除
            </button>
          </li>
        ))}
      </ul>
    </main>
  );
}
