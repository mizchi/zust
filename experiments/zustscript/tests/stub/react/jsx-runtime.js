// A stand-in for react/jsx-runtime: an element is { type, props, key }; render() turns it into HTML
export const Fragment = Symbol("Fragment");
export const jsx = (type, props, key) => ({ type, props, key });
export const jsxs = jsx;
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;");
export function render(el) {
  if (el == null || el === false) return "";
  if (Array.isArray(el)) return el.map(render).join("");
  if (typeof el !== "object") return esc(el);
  if (typeof el.type === "function") return render(el.type(el.props));
  const kids = render(el.props.children);
  if (el.type === Fragment) return kids;
  const attrs = Object.entries(el.props)
    .filter(([k, v]) => k !== "children" && typeof v !== "function")
    .map(([k, v]) => ` ${k === "className" ? "class" : k}="${esc(v)}"`).join("");
  const handlers = Object.keys(el.props).filter((k) => typeof el.props[k] === "function");
  return `<${el.type}${attrs}${handlers.length ? ` data-on="${handlers.join(",")}"` : ""}>${kids}</${el.type}>`;
}
