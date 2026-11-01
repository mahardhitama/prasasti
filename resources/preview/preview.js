// Renders math (KaTeX) and Mermaid (Pele) in place. A module script runs once
// the document is parsed. One block's failure never affects the others.
import { render } from "./pele/pele.js";

try {
  for (const el of document.querySelectorAll(".math-inline, .math-display")) {
    try {
      katex.render(el.textContent, el, {
        throwOnError: false,
        displayMode: el.classList.contains("math-display"),
      });
    } catch (e) {
      el.insertAdjacentElement("afterend", errorBox(e));
    }
  }
  for (const code of document.querySelectorAll("pre > code.language-mermaid")) {
    const pre = code.parentElement;
    try {
      const div = document.createElement("div");
      div.className = "mermaid";
      div.innerHTML = render(code.textContent).svg;
      pre.replaceWith(div);
    } catch (e) {
      pre.insertAdjacentElement("afterend", errorBox(e));
    }
  }
} finally {
  document.body.dataset.rendered = "1";
}

function errorBox(e) {
  const div = document.createElement("div");
  div.className = "render-error";
  div.textContent = (e && e.message) || String(e);
  return div;
}
