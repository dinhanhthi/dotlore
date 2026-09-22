const LIGHTS = [
  { label: "Close", className: "bg-[#FF5F57] shadow-[inset_0_0_0_0.5px_#e0443e]", glyph: "×" },
  { label: "Minimize", className: "bg-[#FEBC2E] shadow-[inset_0_0_0_0.5px_#dea123]", glyph: "−" },
  { label: "Zoom", className: "bg-[#28C840] shadow-[inset_0_0_0_0.5px_#1aab29]", glyph: "+" },
] as const;

/** Decorative macOS window chrome. Native traffic lights sit here in the Tauri app. */
export function TrafficLights() {
  return (
    <div
      role="group"
      aria-label="Window controls"
      className="group/traffic fixed top-0 left-0 z-50 flex h-titlebar items-center gap-2 pl-5"
    >
      {LIGHTS.map((light) => (
        <button
          key={light.label}
          type="button"
          aria-label={light.label}
          className={`relative size-3 rounded-full ${light.className}`}
        >
          <span aria-hidden className="absolute inset-0 grid place-items-center text-[9px] leading-none font-bold text-black/55 opacity-0 group-hover/traffic:opacity-100">
            {light.glyph}
          </span>
        </button>
      ))}
    </div>
  );
}
