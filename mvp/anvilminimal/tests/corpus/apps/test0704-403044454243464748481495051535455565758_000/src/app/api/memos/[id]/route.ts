import { NextResponse } from 'next/server';
import { getMemos, saveMemos } from '@/lib/storage';

export async function DELETE(
  request: Request,
  { params }: { params: { id: string } }
) {
  const memos = getMemos();
  const filtered = memos.filter((m) => m.id !== params.id);
  saveMemos(filtered);
  return NextResponse.json({ success: true });
}
