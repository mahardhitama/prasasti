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
  // After Pele: its blocks are gone, and an errored ```mermaid stays plain.
  for (const code of document.querySelectorAll("pre > code:not(.language-mermaid)")) {
    try {
      decorate(code);
    } catch (e) {
      console.error(e);
    }
  }
} finally {
  document.body.dataset.rendered = "1";
}

// Highlights a tagged block (no auto-detection), then wraps each line in a
// span: CSS numbers the lines, and leading whitespace gets indent guides.
function decorate(code) {
  const text = code.textContent.replace(/\n$/, "");
  const lang = /(?:^|\s)language-(\S+)/.exec(code.className)?.[1];
  let html;
  if (lang && typeof hljs !== "undefined" && hljs.getLanguage(lang)) {
    html = hljs.highlight(text, { language: lang, ignoreIllegals: true }).value;
  } else {
    const d = document.createElement("div");
    d.textContent = text;
    html = d.innerHTML;
  }
  // Close and reopen spans at each newline so every line is balanced.
  const lines = [];
  const open = [];
  let cur = "";
  for (const tok of html.match(/<span[^>]*>|<\/span>|\n|[^<\n]+/g) ?? []) {
    if (tok === "\n") {
      lines.push(cur + "</span>".repeat(open.length));
      cur = open.join("");
    } else {
      if (tok.startsWith("<span")) open.push(tok);
      else if (tok === "</span>") open.pop();
      cur += tok;
    }
  }
  lines.push(cur);
  const widths = text.split("\n").map((l) => /^[ \t]*/.exec(l)[0]);
  const spaces = widths.filter((w) => w && !w.includes("\t")).map((w) => w.length);
  const tabbed = widths.some((w) => w.includes("\t"));
  code.style.setProperty("--u", tabbed || spaces.every((n) => n % 4 === 0) ? 4 : 2);
  code.style.setProperty("--gw", String(lines.length).length);
  code.innerHTML = lines
    .map((l) => `<span class="line">${l.replace(/^[ \t]+/, '<span class="ind">$&</span>')}</span>`)
    .join("\n");
}

function errorBox(e) {
  const div = document.createElement("div");
  div.className = "render-error";
  div.textContent = (e && e.message) || String(e);
  return div;
}
