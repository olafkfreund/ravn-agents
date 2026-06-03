import { useTheme } from "../lib/theme";

export function ThemeToggle() {
  const [theme, toggle] = useTheme();
  const next = theme === "dark" ? "light" : "dark";
  return (
    <button
      type="button"
      onClick={toggle}
      aria-label={`Switch to ${next} theme`}
      title={`Switch to ${next} theme`}
      className="inline-flex items-center gap-1.5 rounded-full border border-border bg-bg-elev px-3 py-1 text-sm text-fg transition-colors hover:border-accent"
    >
      <span aria-hidden="true">{theme === "dark" ? "☀" : "☾"}</span>
      <span>{next === "dark" ? "Dark" : "Light"}</span>
    </button>
  );
}
