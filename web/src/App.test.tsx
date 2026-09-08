import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import App from "./App";

test("renders the home page", () => {
  const html = renderToStaticMarkup(<App />);

  expect(html).toContain('href="/"');
  expect(html).toMatch(/<h1\b[^>]*>Lattis<\/h1>/);
});
