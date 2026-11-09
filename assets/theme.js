// Picks light or dark before the page is drawn, so it does not flash.
// garden.js keeps following the system setting afterwards.
document.documentElement.dataset.theme = matchMedia("(prefers-color-scheme: dark)").matches
  ? "glass-dark"
  : "glass-light";
