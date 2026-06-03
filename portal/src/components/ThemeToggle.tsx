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
      className="grid h-9 w-9 place-items-center rounded-lg border border-line text-fg-dim transition-colors hover:border-accent hover:text-fg"
    >
      <span aria-hidden="true" className="text-sm">
        {theme === "dark" ? "☀" : "☾"}
      </span>
    </button>
  );
}
