// Sidebar: the drawer on narrow screens (menu button, scrim, Escape) and the section of this page in view.
(() => {
  const side = document.getElementById('side');
  const btn = document.getElementById('menu');
  const scrim = document.getElementById('scrim');
  if (!side || !btn || !scrim) return;
  // (while the drawer is open, the rest of the page is inert: keyboard focus stays in the drawer, and goes back to the
  //  menu button when it closes, unless a link in it was followed)
  // (the siblings of the drawer and of each of its ancestors: the header, and the content beside it in .shell)
  const rest = () => {
    const out = [];
    for (let n = side; n && n !== document.body; n = n.parentElement) {
      for (const e of n.parentElement.children) {
        if (e !== n && e !== scrim && e.tagName !== 'SCRIPT') out.push(e);
      }
    }
    return out;
  };
  const set = (open, refocus = true) => {
    const was = side.classList.contains('open');
    side.classList.toggle('open', open);
    scrim.hidden = !open;
    btn.setAttribute('aria-expanded', String(open));
    rest().forEach((e) => { e.inert = open; });
    if (open) side.querySelector('a')?.focus();
    else if (was && refocus) btn.focus();
  };
  btn.addEventListener('click', () => set(!side.classList.contains('open')));
  scrim.addEventListener('click', () => set(false));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && side.classList.contains('open')) set(false); });
  side.addEventListener('click', (e) => { if (e.target.closest('a') && side.classList.contains('open')) set(false, false); });
  // (widened to the desktop layout, where the sidebar is always shown: no drawer, no scrim)
  const wide = window.matchMedia('(min-width: 1000px)');
  wide.addEventListener('change', (e) => { if (e.matches && side.classList.contains('open')) set(false, false); });

  const links = [...side.querySelectorAll('.toc a[href^="#"]')];
  const targets = links.map((a) => document.getElementById(a.getAttribute('href').slice(1))).filter(Boolean);
  if (!targets.length || !('IntersectionObserver' in window)) return;
  const visible = new Map();
  const mark = () => {
    const top = targets.find((t) => visible.get(t));
    links.forEach((a) => a.classList.toggle('active', Boolean(top) && a.getAttribute('href') === '#' + top.id));
  };
  const io = new IntersectionObserver((es) => { es.forEach((e) => visible.set(e.target, e.isIntersecting)); mark(); },
    { rootMargin: '-20% 0px -60% 0px' });
  targets.forEach((t) => io.observe(t));
})();
