// Tiny DOM builders shared by the panels.

export type Child = Node | string | null | undefined | false;

/** `h("div", "class", child, ...)`; falsy children are skipped. */
export function h<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, ...children: Child[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  for (const c of children) if (c) e.append(c);
  return e;
}

export function button(text: string, onClick: () => void, cls?: string): HTMLButtonElement {
  const b = h("button", cls, text);
  b.type = "button";
  b.addEventListener("click", onClick);
  return b;
}
