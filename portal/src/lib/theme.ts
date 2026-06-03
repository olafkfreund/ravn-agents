import { useEffect, useState } from "react";

export type Theme = "light" | "dark";

const KEY = "ravn-theme";

function currentTheme(): Theme {
  const attr = document.documentElement.getAttribute("data-theme");
  return attr === "light" ? "light" : "dark";
}

/** Hook for reading and toggling the light/dark theme. */
export function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(currentTheme);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    try {
      localStorage.setItem(KEY, theme);
    } catch {
      /* ignore */
    }
  }, [theme]);

  const toggle = () => setTheme((t) => (t === "dark" ? "light" : "dark"));
  return [theme, toggle];
}
