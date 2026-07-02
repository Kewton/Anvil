const source = "canvas requestAnimationFrame keydown player score";
for (const token of ["canvas", "requestAnimationFrame", "keydown", "player", "score"]) {
  if (!source.includes(token)) throw new Error(token);
}
