// Light/dark theme toggle. The initial theme is set by an inline script in
// <head> (to avoid a flash); this wires the button and persists the choice.
(function () {
  var btn = document.getElementById('theme-toggle');
  if (!btn) return;

  var label = btn.querySelector('.theme-toggle__label');

  function current() {
    return document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'light';
  }

  function apply(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    try { localStorage.setItem('theme', theme); } catch (e) { /* ignore */ }
    btn.setAttribute('aria-pressed', theme === 'dark' ? 'true' : 'false');
    if (label) label.textContent = theme === 'dark' ? 'Light' : 'Dark';
  }

  // Sync the button to whatever the head script picked.
  apply(current());

  btn.addEventListener('click', function () {
    apply(current() === 'dark' ? 'light' : 'dark');
  });
})();
