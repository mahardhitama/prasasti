// Renders math (KaTeX) and Mermaid (Pele) in place. A module script runs once
// the document is parsed. One block's failure never affects the others.
import { render } from "./pele/pele.js";

const COPY_ICON =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="5.5" y="5.5" width="8" height="8" rx="2"/><path d="M10.5 3.5v-.5a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 3v5A1.5 1.5 0 0 0 4 9.5h.5"/></svg>';
const CHECK_ICON =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5l3.2 3.2L13 4.8"/></svg>';

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
      addCopyButton(code.parentElement);
    } catch (e) {
      console.error(e);
    }
  }
} finally {
  document.body.dataset.rendered = "1";
}

// Highlights a tagged block (no auto-detection), then wraps each line in a
// span: CSS numbers the lines.
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
  code.style.setProperty("--gw", String(lines.length).length);
  code.innerHTML = lines
    .map((l) => `<span class="line">${l}</span>`)
    .join("\n");
}

// The button sits in a wrapper beside the <pre>, so it neither scrolls with
// long lines nor lands in a selection of the code. Copying goes through native
// code: a custom scheme is no secure context for navigator.clipboard.
function addCopyButton(pre) {
  const wrap = document.createElement("div");
  wrap.className = "code-block";
  pre.replaceWith(wrap);
  wrap.append(pre);
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "copy-code";
  btn.setAttribute("aria-label", "Copy code");
  btn.title = "Copy";
  btn.innerHTML = COPY_ICON;
  let timer;
  btn.addEventListener("click", () => {
    window.webkit.messageHandlers.prasasti.postMessage({ type: "copy", text: pre.textContent });
    btn.innerHTML = CHECK_ICON;
    clearTimeout(timer);
    timer = setTimeout(() => (btn.innerHTML = COPY_ICON), 1500);
  });
  wrap.append(btn);
}

function errorBox(e) {
  const div = document.createElement("div");
  div.className = "render-error";
  div.textContent = (e && e.message) || String(e);
  return div;
}
