import { describe, it, expect, beforeEach, vi } from 'vitest';
import { getMemos, saveMemo, deleteMemo } from './memoService';

// Mock localStorage
const localStorageMock = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] || null,
    setItem: (key: string, value: string) => {
      store[key] = value.toString();
    },
    clear: () => {
      store = {};
    },
  };
})();

Object.defineProperty(window, 'localStorage', {
  value: localStorageMock,
});

describe('memoService', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('should save and retrieve a memo', () => {
    const memo = { id: '1', content: 'test memo' };
    saveMemo(memo);
    const memos = getMemos();
    expect(memos).toHaveLength(1);
    expect(memos[0].content).toBe('test memo');
  });

  it('should delete a memo', () => {
    saveMemo({ id: '1', content: 'test memo' });
    deleteMemo('1');
    const memos = getMemos();
    expect(memos).toHaveLength(0);
  });
});
