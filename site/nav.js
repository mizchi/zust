// Sidebar: the drawer on narrow screens (menu button, scrim, Escape) and the section of this page in view.
(() => {
  const side = document.getElementById('side');
  const btn = document.getElementById('menu');
  const scrim = document.getElementById('scrim');
  if (!side || !btn || !scrim) return;
  const set = (open) => {
    side.classList.toggle('open', open);
    scrim.hidden = !open;
    btn.setAttribute('aria-expanded', String(open));
    if (open) side.querySelector('a')?.focus();
  };
  btn.addEventListener('click', () => set(!side.classList.contains('open')));
  scrim.addEventListener('click', () => set(false));
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && side.classList.contains('open')) { set(false); btn.focus(); } });
  side.addEventListener('click', (e) => { if (e.target.closest('a') && side.classList.contains('open')) set(false); });

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
