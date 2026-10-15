// Apply each app's existing theme preference before its WASM starts.
// This only paints the initial theme; each app keeps its own theme controls.
(function () {
  try {
    var key = document.currentScript.getAttribute("data-hagency-storage");
    var stored = localStorage.getItem(key);
    var resolved = stored === "dark" ||
      (stored !== "light" && matchMedia("(prefers-color-scheme: dark)").matches)
      ? "dark" : "light";
    var root = document.documentElement;
    if (key === "pasion_theme") {
      root.setAttribute("data-theme", resolved);
    } else {
      root.classList.remove("light", "dark");
      root.classList.add(resolved);
    }
  } catch (_) {
    // The stylesheet still follows the system preference if storage is blocked.
  }
})();
