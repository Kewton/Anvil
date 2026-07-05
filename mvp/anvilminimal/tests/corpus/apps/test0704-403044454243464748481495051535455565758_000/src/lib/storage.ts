import fs from 'fs';
import path from 'path';

const DB_PATH = path.join(process.cwd(), 'data.json');

export interface Memo {
  id: string;
  content: string;
  createdAt: number;
}

const ensureDb = () => {
  if (!fs.existsSync(DB_PATH)) {
    fs.writeFileSync(DB_PATH, JSON.stringify([]));
  }
};

export const getMemos = (): Memo[] => {
  ensureDb();
  return JSON.parse(fs.readFileSync(DB_PATH, 'utf-8'));
};

export const saveMemos = (memos: Memo[]) => {
  fs.writeFileSync(DB_PATH, JSON.stringify(memos, null, 2));
};
