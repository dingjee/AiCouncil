import type { JSX } from "react";

/**
 * Inline SVG icons.
 *
 * Icons are bundled with the app (no icon font, no CDN) and inherit the
 * current text colour so they follow the Soft Sage palette.
 */

export type IconName =
  | "plus"
  | "search"
  | "pencil"
  | "trash"
  | "folder"
  | "folderPlus"
  | "send"
  | "stop"
  | "retry"
  | "copy"
  | "branch"
  | "split"
  | "models"
  | "settings"
  | "chats"
  | "sidebar"
  | "export"
  | "star"
  | "starFilled"
  | "check"
  | "paperclip"
  | "down"
  | "close"
  | "branchSelect";

const PATHS: Record<IconName, JSX.Element> = {
  plus: <path d="M8 3v10M3 8h10" />,
  search: (
    <>
      <circle cx="7" cy="7" r="4" />
      <path d="M10 10l4 4" />
    </>
  ),
  pencil: <path d="M11 2l3 3-8 8-4 1 1-4 8-8z" />,
  trash: (
    <>
      <path d="M3 4h10M6 4V2h4v2M4 4l1 10h6l1-10" />
    </>
  ),
  folder: <path d="M2 4h4l1 2h7v8H2z" />,
  folderPlus: (
    <>
      <path d="M2 4h4l1 2h7v8H2z" />
      <path d="M8.5 8v4M6.5 10h4" />
    </>
  ),
  send: <path d="M2 8l12-5-5 12-2-5-5-2z" />,
  stop: <rect x="4" y="4" width="8" height="8" rx="1" />,
  retry: (
    <>
      <path d="M13 8a5 5 0 1 1-2-4" />
      <path d="M13 2v4H9" />
    </>
  ),
  copy: (
    <>
      <rect x="5" y="5" width="8" height="8" rx="1" />
      <path d="M3 10V3h7" />
    </>
  ),
  branch: (
    <>
      <path d="M4 3v4a2 2 0 0 0 2 2h6" />
      <path d="M4 3v8" />
      <circle cx="4" cy="12" r="1.6" />
      <circle cx="12" cy="9" r="1.6" />
    </>
  ),
  branchSelect: (
    <>
      <circle cx="4" cy="4" r="1.8" />
      <circle cx="12" cy="8" r="1.8" />
      <circle cx="4" cy="12" r="1.8" />
      <path d="M4 6v4M5.6 8H10" />
    </>
  ),
  split: (
    <>
      <rect x="2" y="3" width="5" height="10" rx="1" />
      <rect x="9" y="3" width="5" height="10" rx="1" />
    </>
  ),
  models: (
    <>
      <circle cx="8" cy="8" r="5" />
      <path d="M8 3v10M3 8h10" />
    </>
  ),
  settings: (
    <>
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4" />
    </>
  ),
  chats: (
    <>
      <path d="M2 4h12v8H7l-3 3v-3H2z" />
    </>
  ),
  sidebar: (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.5" />
      <path d="M6 3v10" />
    </>
  ),
  export: (
    <>
      <path d="M8 2v8" />
      <path d="M5 7l3 3 3-3" />
      <path d="M3 12h10" />
    </>
  ),
  star: <path d="M8 2l1.8 3.8 4.2.5-3.1 2.9.8 4.2L8 11.3 4.3 13.4l.8-4.2L2 6.3l4.2-.5z" />,
  starFilled: (
    <path d="M8 2l1.8 3.8 4.2.5-3.1 2.9.8 4.2L8 11.3 4.3 13.4l.8-4.2L2 6.3l4.2-.5z" />
  ),
  check: <path d="M3 8.5l3.2 3.2L13 5" />,
  paperclip: <path d="M11 7l-4.2 4.2a2.1 2.1 0 0 1-3-3L8 4a1.4 1.4 0 0 1 2 2l-4 4a.7.7 0 0 1-1-1l4-4" />,
  down: <path d="M8 3v8M5 8.5l3 3 3-3" />,
  close: <path d="M4 4l8 8M12 4l-8 8" />,
};

export interface IconProps {
  name: IconName;
  size?: number;
  className?: string;
  title?: string;
}

export function Icon({ name, size = 16, className, title }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      role={title ? "img" : "presentation"}
      aria-hidden={title ? undefined : true}
      aria-label={title}
      focusable="false"
    >
      {title ? <title>{title}</title> : null}
      {PATHS[name]}
    </svg>
  );
}
