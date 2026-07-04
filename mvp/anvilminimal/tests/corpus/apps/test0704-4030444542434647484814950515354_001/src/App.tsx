import React, { useState } from 'react'

export default function App() {
  const [memo, setMemo] = useState('')

  return (
    <div className="min-h-screen bg-gray-100 p-4">
      <header className="bg-white p-4 shadow rounded mb-4">
        <h1 className="text-2xl font-bold text-gray-800">My Memo App</h1>
      </header>
      <main className="bg-white p-4 shadow rounded">
        <textarea
          className="w-full h-64 p-2 border border-gray-300 rounded"
          placeholder="Write your memo here..."
          value={memo}
          onChange={(e) => setMemo(e.target.value)}
        />
      </main>
    </div>
  )
}
