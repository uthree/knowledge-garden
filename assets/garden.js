// Small enhancements; every page works without them.
import { followSystemAppearance, hydrate } from "./atelier/atelier.js";

followSystemAppearance();
hydrate();

// Times are rendered in UTC; show them in the reader's time zone.
const format = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
for (const t of document.querySelectorAll("time[datetime]")) {
  const d = new Date(t.getAttribute("datetime"));
  if (!Number.isNaN(d.getTime())) {
    t.title = t.textContent;
    t.textContent = format.format(d);
  }
}

// Show the reproduction fields only for reproduction reports.
for (const select of document.querySelectorAll("select[data-kg-toggle]")) {
  const name = select.dataset.kgToggle;
  const targets = document.querySelectorAll(`[data-kg-when="${name}"]`);
  const update = () => {
    const on = select.value === "reproduction";
    for (const t of targets) t.hidden = !on;
  };
  select.addEventListener("change", update);
  update();
}

// Fill an empty body with the recommended headings of the chosen kind.
for (const select of document.querySelectorAll("select[data-kg-template]")) {
  const body = document.getElementById(select.dataset.kgTemplate);
  if (!body) continue;
  let filled = "";
  const fill = () => {
    if (body.value.trim() !== "" && body.value !== filled) return;
    filled = select.selectedOptions[0]?.dataset.template ?? "";
    body.value = filled;
  };
  select.addEventListener("change", fill);
  fill();
}

// Copy buttons for secrets shown once.
for (const button of document.querySelectorAll("[data-kg-copy]")) {
  button.addEventListener("click", async () => {
    const source = document.getElementById(button.dataset.kgCopy);
    if (!source) return;
    try {
      await navigator.clipboard.writeText(source.textContent.trim());
      button.lastChild.textContent = "コピーしました";
    } catch {
      getSelection().selectAllChildren(source);
    }
  });
}

// On narrow windows the sidebar opens from the menu button.
const toggle = document.querySelector(".kg-nav-toggle");
toggle?.addEventListener("click", () => {
  const open = document.body.classList.toggle("kg-nav-open");
  toggle.setAttribute("aria-expanded", String(open));
});
