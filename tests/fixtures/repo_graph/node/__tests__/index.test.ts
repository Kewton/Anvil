import { greet } from "../src/index";

test("greets", () => {
  expect(greet("anvil")).toContain("anvil");
});
