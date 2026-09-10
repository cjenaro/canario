// Theme helpers — apply the persisted theme to the document root.
// Theme values: "dark" | "light" | "system" (system = no data-theme attribute).

export function applyTheme(t: string) {
  const root = document.documentElement;
  if (t === "light") {
    root.style.setProperty("color-scheme", "light");
    root.setAttribute("data-theme", "light");
  } else if (t === "system") {
    root.style.removeProperty("color-scheme");
    root.removeAttribute("data-theme");
  } else {
    root.style.setProperty("color-scheme", "dark");
    root.setAttribute("data-theme", "dark");
  }
}

/** Fetch the persisted theme from the main process and apply it. */
export async function loadAndApplyTheme(): Promise<string> {
  let theme = "dark";
  try {
    theme = (await window.canario?.getTheme?.()) || "dark";
  } catch {
    // Not running in Electron — fall back to dark
  }
  applyTheme(theme);
  return theme;
}
