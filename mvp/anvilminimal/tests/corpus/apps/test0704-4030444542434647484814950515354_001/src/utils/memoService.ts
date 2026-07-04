export interface Memo {
  id: string;
  content: string;
  updatedAt: number;
}

const STORAGE_KEY = 'memos';

export const getMemos = (): Memo[] => {
  const data = localStorage.getItem(STORAGE_KEY);
  return data ? JSON.parse(data) : [];
};

export const saveMemo = (memo: Omit<Memo, 'updatedAt'>): Memo => {
  const memos = getMemos();
  const index = memos.findIndex((m) => m.id === memo.id);
  const newMemo: Memo = { ...memo, updatedAt: Date.now() };

  if (index >= 0) {
    memos[index] = newMemo;
  } else {
    memos.push(newMemo);
  }
  localStorage.setItem(STORAGE_KEY, JSON.stringify(memos));
  return newMemo;
};

export const deleteMemo = (id: string): void => {
  const memos = getMemos().filter((m) => m.id !== id);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(memos));
};
