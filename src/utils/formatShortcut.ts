export function formatShortcutLabel(shortcut: string): string {
  if (!shortcut) return "";
  return shortcut
    .split("+")
    .map((part) => {
      const trimmed = part.trim();
      switch (trimmed.toLowerCase()) {
        case "commandorcontrol":
          return "Cmd/Ctrl";
        case "command":
        case "cmd":
        case "meta":
        case "super":
          return "Cmd";
        case "control":
        case "ctrl":
          return "Ctrl";
        case "alt":
        case "option":
          return "Alt";
        case "shift":
          return "Shift";
        default:
          return trimmed;
      }
    })
    .filter(Boolean)
    .join(" + ");
}
