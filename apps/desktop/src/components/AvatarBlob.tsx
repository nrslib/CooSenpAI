import type { CSSProperties, ReactElement } from "react";

interface Props {
  readonly color?: string | null;
  readonly image?: readonly number[];
  readonly size: 24 | 28 | 40;
  readonly state?: "open" | "resting" | "thinking";
  readonly animated?: boolean;
  readonly squashed?: boolean;
}

export function AvatarBlob({ color, image, size, state = "open", animated = false, squashed = false }: Props): ReactElement {
  const style = {
    "--avatar-color": color ?? "var(--logo-body)",
    "--avatar-size": `${size}px`,
  } as CSSProperties;
  const animatedClass = animated ? " avatar-animated" : "";
  const imageUrl = avatarImageDataUrl(image);
  return <span className={`avatar-blob avatar-${state}${animatedClass}${squashed ? " avatar-squashed" : ""}`} style={style} aria-hidden="true">
    {imageUrl === undefined ? <svg viewBox="0 0 100 100">
      <g className="avatar-mark-body" transform={squashed ? "translate(0 10) scale(1 .8)" : undefined}>
        <g transform="rotate(135 50 50)">
          <path d="M76 19C67 10 55 7 43 9C23 12 9 29 9 50C9 71 23 88 44 91C58 93 70 89 79 80C86 73 84 62 75 60C71 59 67 61 63 65C58 70 52 73 45 72C34 70 27 61 27 50C27 38 34 30 45 28C53 27 59 29 64 35C68 39 72 41 76 40C85 38 86 27 76 19Z" />
        </g>
        <ellipse cx="20" cy="81" rx="9.5" ry="6" />
        <ellipse cx="7.5" cy="92" rx="5" ry="3.2" />
      </g>
      <g className="avatar-eye-state avatar-eyes-open"><g className="avatar-eye-primary avatar-mark-eye"><circle cx="44" cy="42" r="6.5" /><circle cx="62" cy="42" r="6.5" /></g></g>
      <g className="avatar-eye-state avatar-eyes-sleep"><g className="avatar-eye-primary avatar-mark-eye-line"><path d="M38 56Q44 59.5 50 56" /><path d="M56 56Q62 59.5 68 56" /></g></g>
      <g className="avatar-blink-state"><g className="avatar-blink-cycle avatar-mark-eye-line"><path d="M38 44Q44 47.5 50 44" /><path d="M56 44Q62 47.5 68 44" /></g></g>
    </svg> : <img className="avatar-image" src={imageUrl} alt="" />}
  </span>;
}

function avatarImageDataUrl(bytes?: readonly number[]): string | undefined {
  if (bytes === undefined || bytes.length === 0 || typeof globalThis.btoa !== "function") {
    return undefined;
  }
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.slice(offset, offset + 0x8000));
  }
  return `data:image/png;base64,${globalThis.btoa(binary)}`;
}
