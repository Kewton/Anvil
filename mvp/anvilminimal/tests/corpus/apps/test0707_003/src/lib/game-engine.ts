export interface GameState {
  score: number;
  lives: number;
  wave: number;
  status: "ready" | "playing" | "paused" | "gameover";
}

export class SpaceInvadersEngine {
  private state: GameState = { score: 0, lives: 3, wave: 1, status: "ready" };
  private running = false;
  private keys = new Set<string>();
  private animationFrame = 0;

  constructor(private readonly canvas: HTMLCanvasElement) {}

  public start() {
    this.running = true;
    this.state = { ...this.state, status: "playing" };
    this.animationFrame = requestAnimationFrame(() => this.tick());
  }

  public pause() {
    this.running = false;
    this.state = { ...this.state, status: "paused" };
    cancelAnimationFrame(this.animationFrame);
  }

  public reset() {
    this.running = false;
    this.keys.clear();
    this.state = { score: 0, lives: 3, wave: 1, status: "ready" };
    this.draw();
  }

  public setKey(key: string, pressed: boolean) {
    if (pressed) {
      this.keys.add(key);
    } else {
      this.keys.delete(key);
    }
  }

  public getState(): GameState {
    return this.state;
  }

  public destroy() {
    this.running = false;
    cancelAnimationFrame(this.animationFrame);
    this.keys.clear();
  }

  private tick() {
    if (!this.running) return;
    const scoreDelta = this.keys.has(" ") ? 10 : 1;
    this.state = { ...this.state, score: this.state.score + scoreDelta };
    this.draw();
    this.animationFrame = requestAnimationFrame(() => this.tick());
  }

  private draw() {
    const context = this.canvas.getContext("2d");
    if (!context) return;
    context.fillStyle = "#07120f";
    context.fillRect(0, 0, this.canvas.width, this.canvas.height);
    context.fillStyle = "#22d3ee";
    context.fillRect(320, 420, 80, 24);
    context.fillStyle = "#f43f5e";
    for (let row = 0; row < 4; row += 1) {
      for (let col = 0; col < 8; col += 1) {
        context.fillRect(120 + col * 56, 80 + row * 42, 32, 24);
      }
    }
  }
}
