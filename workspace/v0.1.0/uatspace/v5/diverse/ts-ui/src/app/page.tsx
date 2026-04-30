"use client";

import { FormEvent, useMemo, useState } from "react";

type Attempt = { id: number; label: string; score: number; elapsed: number; note: string };
const storageKey = "anvil:interactive-practice-history";
const initialHistory: Attempt[] = [
  { id: 1, label: "Baseline", score: 74, elapsed: 18.4, note: "Initial reference run" },
  { id: 2, label: "Clean run", score: 88, elapsed: 14.2, note: "Faster and more accurate" },
];

export default function App() {
  const [running, setRunning] = useState(false);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [entry, setEntry] = useState("");
  const [history, setHistory] = useState<Attempt[]>(() => {
    if (typeof window === "undefined") return initialHistory;
    try {
      const saved = window.localStorage.getItem(storageKey);
      return saved ? JSON.parse(saved) as Attempt[] : initialHistory;
    } catch {
      return initialHistory;
    }
  });
  const [laps, setLaps] = useState<number[]>([18.4, 14.2]);
  const [error, setError] = useState("");

  const best = useMemo(() => history.reduce<Attempt | null>((winner, attempt) => {
    if (!winner) return attempt;
    if (attempt.score > winner.score) return attempt;
    if (attempt.score === winner.score && attempt.elapsed < winner.elapsed) return attempt;
    return winner;
  }, null), [history]);
  const averageScore = useMemo(() => Math.round(history.reduce((sum, attempt) => sum + attempt.score, 0) / history.length), [history]);
  const projectedTotal = useMemo(() => {
    const unitPrice = 1200;
    const quantity = Math.max(1, history.length);
    const discountRate = 0.12;
    return Math.round(unitPrice * quantity * (1 - discountRate));
  }, [history.length]);
  const targetMet = projectedTotal >= 5000;
  const targetProgress = Math.max(8, Math.min(100, Math.round((projectedTotal / 5000) * 100)));

  function beginRun() {
    setRunning(true);
    setStartedAt(Date.now());
    setElapsed(0);
    setError("");
  }

  function recordLap() {
    if (!running || startedAt === null) return;
    const seconds = Number(((Date.now() - startedAt) / 1000).toFixed(1));
    setElapsed(seconds);
    setLaps((current) => [seconds, ...current].slice(0, 6));
  }

  function finishRun(event?: FormEvent) {
    event?.preventDefault();
    const text = entry.trim();
    if (!text) {
      setError("Enter a result or observation before saving.");
      return;
    }
    const seconds = startedAt === null ? elapsed || 1 : Number(((Date.now() - startedAt) / 1000).toFixed(1));
    const score = Math.max(1, Math.min(100, 60 + text.length * 2 - Math.round(seconds / 2)));
    setHistory((current) => {
      const next = [
        { id: Date.now(), label: `Run ${current.length + 1}`, score, elapsed: seconds, note: text },
        ...current,
      ].slice(0, 8);
      try {
        window.localStorage.setItem(storageKey, JSON.stringify(next));
      } catch {}
      return next;
    });
    setLaps((current) => [seconds, ...current].slice(0, 6));
    setEntry("");
    setElapsed(seconds);
    setRunning(false);
    setStartedAt(null);
    setError("");
  }

  return (
    <main style={{ minHeight: "100vh", padding: 32, background: "#f7f8fb", color: "#172033", fontFamily: "Inter, system-ui, sans-serif" }}>
      <section style={{ maxWidth: 960, margin: "0 auto", display: "grid", gap: 20 }}>
        <header style={{ display: "flex", justifyContent: "space-between", gap: 16, alignItems: "end", borderBottom: "1px solid #d8deea", paddingBottom: 16 }}>
          <div>
            <p style={{ margin: 0, color: "#58657a", fontWeight: 700 }}>Practice Console</p>
            <h1 style={{ margin: 0, fontSize: 40 }}>Interactive Practice App</h1>
          </div>
          <strong aria-live="polite">Best {best?.score ?? 0} / {best?.elapsed.toFixed(1) ?? "0.0"}s</strong>
        </header>

        <section style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(170px, 1fr))", gap: 10 }}>
          <article style={{ padding: 16, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}><strong>{history.length}</strong><p style={{ margin: "6px 0 0", color: "#58657a" }}>score history</p></article>
          <article style={{ padding: 16, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}><strong>{averageScore}</strong><p style={{ margin: "6px 0 0", color: "#58657a" }}>average score</p></article>
          <article style={{ padding: 16, border: "1px solid #d8deea", borderRadius: 8, background: "white" }}><strong>{running ? "Running" : "Ready"}</strong><p style={{ margin: "6px 0 0", color: "#58657a" }}>current state</p></article>
        </section>

        <section style={{ display: "grid", gridTemplateColumns: "minmax(280px, 1fr) minmax(260px, 0.8fr)", gap: 14 }}>
          <form onSubmit={finishRun} style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8, display: "grid", gap: 12 }}>
            <h2 style={{ margin: 0 }}>Run panel</h2>
            <p style={{ margin: 0, color: "#58657a" }}>Start a run, record a lap or reaction, and save the result.</p>
            <output aria-live="polite" style={{ fontSize: 44, fontWeight: 900 }}>{elapsed.toFixed(1)}s</output>
            <textarea value={entry} onChange={(event) => setEntry(event.target.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") finishRun(); }} rows={4} aria-label="Run result" placeholder="Type the result, reaction note, lap memo, or practice outcome" style={{ padding: 12, border: "1px solid #c7d0df", borderRadius: 6 }} />
            {error && <p role="alert" style={{ margin: 0, color: "#b42318", fontWeight: 700 }}>{error}</p>}
            <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
              <button type="button" onClick={beginRun} style={{ padding: "10px 16px", border: 0, borderRadius: 6, background: "#172033", color: "white", fontWeight: 800 }}>Start</button>
              <button type="button" onClick={recordLap} style={{ padding: "10px 16px", border: "1px solid #c9d2e3", borderRadius: 6, background: "white", fontWeight: 800 }}>Lap</button>
              <button type="button" onClick={() => { setHistory(initialHistory); try { window.localStorage.setItem(storageKey, JSON.stringify(initialHistory)); } catch {} }} style={{ padding: "10px 16px", border: "1px solid #c9d2e3", borderRadius: 6, background: "white", fontWeight: 800 }}>Restore sample</button>
              <button type="submit" style={{ padding: "10px 16px", border: 0, borderRadius: 6, background: "#2457d6", color: "white", fontWeight: 800 }}>Save score</button>
            </div>
          </form>

          <aside style={{ display: "grid", gap: 12 }}>
            <section style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <h2 style={{ marginTop: 0 }}>Functional check</h2>
              <p style={{ margin: "0 0 8px", color: "#58657a" }}>Calculated total</p>
              <strong>{projectedTotal.toLocaleString()} / Target 5,000</strong>
              <div aria-label="Target progress" style={{ height: 10, background: "#edf1f7", borderRadius: 999, overflow: "hidden", marginTop: 10 }}>
                <span style={{ display: "block", width: `${targetProgress}%`, height: "100%", background: targetMet ? "#14804a" : "#c2410c" }} />
              </div>
              <p aria-live="polite" style={{ marginBottom: 0, fontWeight: 800 }}>{targetMet ? "Target met" : "Needs attention"} with validation guard</p>
            </section>
            <section style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <h2 style={{ marginTop: 0 }}>Score history</h2>
              {history.map((attempt) => (
                <article key={attempt.id} style={{ padding: 10, borderTop: "1px solid #eef1f6" }}>
                  <strong>{attempt.label}: {attempt.score}</strong>
                  <p style={{ margin: "4px 0", color: "#58657a" }}>{attempt.elapsed.toFixed(1)}s / {attempt.note}</p>
                </article>
              ))}
            </section>
            <section style={{ padding: 18, background: "white", border: "1px solid #d8deea", borderRadius: 8 }}>
              <h2 style={{ marginTop: 0 }}>Lap log</h2>
              {laps.map((lap, index) => <p key={index} style={{ margin: "6px 0" }}>Lap {index + 1}: {lap.toFixed(1)}s</p>)}
            </section>
          </aside>
        </section>
      </section>
    </main>
  );
}
