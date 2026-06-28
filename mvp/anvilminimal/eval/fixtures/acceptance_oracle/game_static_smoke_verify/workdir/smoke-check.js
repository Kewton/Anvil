const source = "canvas requestAnimationFrame keydown player enemy collision score lives";
for (const token of ["canvas", "requestAnimationFrame", "keydown", "player", "enemy", "collision", "score", "lives"]) {
  if (!source.includes(token)) throw new Error(token);
}
