import SpaceInvaders from "@/components/SpaceInvaders";

export default function Page() {
  return (
    <main className="flex min-h-screen flex-col items-center justify-center p-8 bg-black text-green-400">
      <h1 className="text-4xl font-bold mb-4">Space Invaders</h1>
      <SpaceInvaders />
    </main>
  );
}
