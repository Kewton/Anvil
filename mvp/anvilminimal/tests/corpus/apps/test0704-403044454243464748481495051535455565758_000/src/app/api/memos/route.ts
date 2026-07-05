import { NextResponse } from 'next/server';
import { getMemos, saveMemos } from '@/lib/storage';

export async function GET() {
  return NextResponse.json(getMemos());
}

export async function POST(request: Request) {
  const { content } = await request.json();
  const memos = getMemos();
  const newMemo = {
    id: Date.now().toString(),
    content,
    createdAt: Date.now(),
  };
  saveMemos([...memos, newMemo]);
  return NextResponse.json(newMemo);
}
