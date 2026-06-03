/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // Channels in CSS vars -> opacity modifiers (bg-accent/10) work.
        bg: "rgb(var(--bg) / <alpha-value>)",
        surface: "rgb(var(--surface) / <alpha-value>)",
        "surface-2": "rgb(var(--surface-2) / <alpha-value>)",
        elev: "rgb(var(--elev) / <alpha-value>)",
        line: "rgb(var(--line) / <alpha-value>)",
        "line-soft": "rgb(var(--line-soft) / <alpha-value>)",
        fg: "rgb(var(--fg) / <alpha-value>)",
        "fg-dim": "rgb(var(--fg-dim) / <alpha-value>)",
        "fg-mute": "rgb(var(--fg-mute) / <alpha-value>)",
        accent: "rgb(var(--accent) / <alpha-value>)",
        "accent-2": "rgb(var(--accent-2) / <alpha-value>)",
        "sev-info": "rgb(var(--sev-info) / <alpha-value>)",
        "sev-notice": "rgb(var(--sev-notice) / <alpha-value>)",
        "sev-warning": "rgb(var(--sev-warning) / <alpha-value>)",
        "sev-error": "rgb(var(--sev-error) / <alpha-value>)",
        "sev-critical": "rgb(var(--sev-critical) / <alpha-value>)",
      },
      fontFamily: {
        display: ['"Bricolage Grotesque"', "ui-sans-serif", "system-ui", "sans-serif"],
        sans: ['"Hanken Grotesk"', "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ['"JetBrains Mono"', "ui-monospace", "SFMono-Regular", "monospace"],
      },
      boxShadow: {
        drawer: "var(--shadow-drawer)",
        card: "var(--shadow-card)",
      },
      keyframes: {
        "fade-up": {
          "0%": { opacity: "0", transform: "translateY(6px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        "slide-in": {
          "0%": { transform: "translateX(100%)" },
          "100%": { transform: "translateX(0)" },
        },
        "fade-in": { "0%": { opacity: "0" }, "100%": { opacity: "1" } },
        "pulse-ring": {
          "0%": { boxShadow: "0 0 0 0 var(--pulse-color)" },
          "70%": { boxShadow: "0 0 0 6px transparent" },
          "100%": { boxShadow: "0 0 0 0 transparent" },
        },
      },
      animation: {
        "fade-up": "fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) both",
        "slide-in": "slide-in 0.28s cubic-bezier(0.22, 1, 0.36, 1) both",
        "fade-in": "fade-in 0.2s ease both",
        "pulse-ring": "pulse-ring 2s ease-out infinite",
      },
    },
  },
  plugins: [],
};
