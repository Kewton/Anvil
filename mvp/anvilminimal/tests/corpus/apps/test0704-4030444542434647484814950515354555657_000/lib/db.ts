import fs from 'fs/promises';
import path from 'path';

const DB_PATH = path.join(process.cwd(), 'db.json');

export interface Memo {
  id: string;
  text: string;
}

async function readDb(): Promise<Memo[]> {
  try {
    const data = await fs.readFile(DB_PATH, 'utf-8');
    return JSON.parse(data);
  } catch (error) {
    return [];
  }
}

async function writeDb(memos: Memo[]): Promise<void> {
  await fs.writeFile(DB_PATH, JSON.stringify(memos, null, 2));
}

export const db = {
  getAll: async () => readDb(),
  add: async (text: string) => {
    const memos = await readDb();
    const newMemo = { id: Date.now().toString(), text };
    memos.push(newMemo);
    await writeDb(memos);
    return newMemo;
  },
  delete: async (id: string) => {
    const memos = await readDb();
    const filtered = memos.filter((m) => m.id !== id);
    await writeDb(filtered);
  },
};
