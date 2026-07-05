import { NextResponse } from 'next/server';
import { db } from '@/lib/db';

export async function GET() {
  const memos = await db.getAll();
  return NextResponse.json(memos);
}

export async function POST(request: Request) {
  const { text } = await request.json();
  const memo = await db.add(text);
  return NextResponse.json(memo);
}

export async function DELETE(request: Request) {
  const { id } = await request.json();
  await db.delete(id);
  return new NextResponse(null, { status: 204 });
}
