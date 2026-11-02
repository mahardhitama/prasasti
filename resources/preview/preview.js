// Renders math (KaTeX) and Mermaid in place. A module script runs once
// the document is parsed. One block's failure never affects the others.

const DARK = matchMedia("(prefers-color-scheme: dark)");
let diagramId = 0;
// Diagrams use the bundled JetBrains Mono, as code blocks do (preview.rs).
const DIAGRAM_FONT = "'Prasasti JetBrains Mono', 'JetBrains Mono', ui-monospace, monospace";

const COPY_ICON =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="5.5" y="5.5" width="8" height="8" rx="2"/><path d="M10.5 3.5v-.5a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 3v5A1.5 1.5 0 0 0 4 9.5h.5"/></svg>';
const CHECK_ICON =
  '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5l3.2 3.2L13 4.8"/></svg>';

// A module runs after parsing, when an image may already have failed:
// recheck finished ones, then catch later failures.
const BROKEN =
  "data:image/svg+xml," +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="160" height="120" viewBox="0 0 160 120"><rect x="1" y="1" width="158" height="118" rx="8" fill="none" stroke="#9ca0b0" stroke-width="2" stroke-dasharray="6 4"/><text x="80" y="58" text-anchor="middle" font-family="-apple-system,sans-serif" font-size="28" font-weight="600" fill="#9ca0b0">404</text><text x="80" y="84" text-anchor="middle" font-family="-apple-system,sans-serif" font-size="12" fill="#9ca0b0">Image not found</text></svg>',
  );
function markBroken(img) {
  if (img.classList.contains("broken")) return;
  img.classList.add("broken");
  img.title ||= img.getAttribute("src") ?? "";
  img.src = BROKEN;
}
document.addEventListener("error", (e) => e.target instanceof HTMLImageElement && markBroken(e.target), true);
for (const img of document.images) {
  if (img.complete && img.naturalWidth === 0) markBroken(img);
}
for (const p of document.querySelectorAll("p")) {
  const nodes = [...p.childNodes].filter((n) => n.nodeType !== Node.TEXT_NODE || n.textContent.trim());
  const only = nodes.length === 1 ? nodes[0] : null;
  if (only?.tagName === "IMG" || (only?.tagName === "A" && only.children.length === 1 && only.firstElementChild.tagName === "IMG" && !only.textContent.trim()))
    p.classList.add("lone-image");
}

// A local image outside a link opens in its default app (preview.rs).
document.addEventListener("click", (e) => {
  const img = e.target instanceof HTMLImageElement ? e.target : null;
  if (!img || img.closest("a") || img.classList.contains("broken") || !img.src.startsWith("prasasti://file/")) return;
  e.preventDefault();
  window.webkit.messageHandlers.prasasti.postMessage({ type: "openImage", src: img.src });
});

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
  const diagrams = document.querySelectorAll("pre > code.language-mermaid");
  if (diagrams.length) {
    try {
      // Mermaid measures labels as it draws, so the font must be ready first.
      await Promise.all([loadMermaid(), document.fonts.load("16px 'Prasasti JetBrains Mono'"), document.fonts.load("bold 16px 'Prasasti JetBrains Mono'")]);
      for (const code of diagrams) {
        const pre = code.parentElement;
        try {
          const div = document.createElement("div");
          div.className = "mermaid";
          div.dataset.src = code.textContent;
          div.innerHTML = await diagramSvg(div.dataset.src);
          pre.replaceWith(div);
        } catch (e) {
          pre.insertAdjacentElement("afterend", errorBox(e));
        }
      }
      // The theme is fixed at render time: redraw when the appearance changes.
      DARK.addEventListener("change", async () => {
        for (const div of document.querySelectorAll(".mermaid")) {
          try {
            div.innerHTML = await diagramSvg(div.dataset.src);
          } catch (e) {
            console.error(e);
          }
        }
      });
    } catch (e) {
      diagrams[0].parentElement.insertAdjacentElement("afterend", errorBox(e));
    }
  }
  // After Mermaid: its blocks are gone, and an errored ```mermaid stays plain.
  for (const code of document.querySelectorAll("pre > code:not(.language-mermaid)")) {
    try {
      decorate(code);
      addCopyButton(code.parentElement);
    } catch (e) {
      console.error(e);
    }
  }
} finally {
  keepScroll();
  document.body.dataset.rendered = "1";
}

// Mermaid is large, so a page loads it only when it has a diagram.
function loadMermaid() {
  return new Promise((resolve, reject) => {
    const s = document.createElement("script");
    s.src = "prasasti://app/mermaid/mermaid.min.js";
    s.onload = resolve;
    s.onerror = () => reject(new Error("Mermaid failed to load"));
    document.head.append(s);
  });
}

async function diagramSvg(src) {
  mermaid.initialize({
    startOnLoad: false,
    suppressErrorRendering: true,
    theme: DARK.matches ? "redux-dark-color" : "redux-color",
    fontFamily: DIAGRAM_FONT,
    themeVariables: { fontFamily: DIAGRAM_FONT },
  });
  return (await mermaid.render(`mermaid-${diagramId++}`, src)).svg;
}

// Every edit re-renders by loading a new page: put the viewport back where it
// was, once now and again when images have their sizes. sessionStorage lives
// as long as this web view, so a new window starts at the top.
function keepScroll() {
  try {
    const y = Number(sessionStorage.getItem("scrollY")) || 0;
    if (y) {
      scrollTo(0, y);
      addEventListener("load", () => scrollTo(0, y), { once: true });
    }
    addEventListener("scroll", () => sessionStorage.setItem("scrollY", String(scrollY)), { passive: true });
  } catch (e) {
    console.error(e);
  }
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
