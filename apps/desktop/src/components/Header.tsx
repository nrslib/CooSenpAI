import { useEffect, useRef, useState, type ReactElement } from "react";

import type { AppSnapshot } from "../types.js";
import { avatarColor, presenceView, stabilizePresence, type StablePresence } from "../view-model.js";
import { AvatarBlob } from "./AvatarBlob.js";

export interface HeaderMenuItem {
  readonly id: string;
  readonly label: string;
  readonly disabled?: boolean;
  readonly onSelect: () => void;
}

export function createHeaderMenuItems(
  onOpenModelPopup: () => void,
  onResetConversation: () => void,
  canResetConversation: boolean,
): readonly HeaderMenuItem[] {
  return [
    { id: "model", label: "モデル変更", onSelect: onOpenModelPopup },
    {
      id: "conversation-reset",
      label: "会話をリセット",
      disabled: !canResetConversation,
      onSelect: onResetConversation,
    },
  ];
}

interface HeaderMenuProps {
  readonly items: readonly HeaderMenuItem[];
  readonly open: boolean;
  readonly onToggle: () => void;
  readonly onSelect: (item: HeaderMenuItem) => void;
}

export function HeaderMenu({ items, open, onToggle, onSelect }: HeaderMenuProps): ReactElement {
  return <div className="header-menu">
    <button
      className="menu-button"
      type="button"
      aria-label={open ? "メニューを閉じる" : "メニューを開く"}
      aria-expanded={open}
      aria-haspopup="menu"
      onClick={onToggle}
    >
      ☰
    </button>
    {open ? <div className="header-menu-list" role="menu" aria-label="チャットメニュー">
      {items.map((item) => <button
        className="header-menu-item"
        key={item.id}
        type="button"
        role="menuitem"
        disabled={item.disabled}
        onClick={() => onSelect(item)}
      >{item.label}</button>)}
    </div> : null}
  </div>;
}

interface Props {
  readonly snapshot: AppSnapshot;
  readonly watchIntentActive: boolean;
  readonly watchChanging: boolean;
  readonly onToggleWatch: () => void;
  readonly audioEnabled?: boolean;
  readonly audioChanging?: boolean;
  readonly onToggleAudio?: () => void;
  readonly onOpenSettings: () => void;
  readonly historyOpen: boolean;
  readonly onToggleHistory: () => void;
  readonly menuItems: readonly HeaderMenuItem[];
}

export function Header({ snapshot, watchIntentActive, watchChanging, onToggleWatch, audioEnabled = false, audioChanging = false, onToggleAudio, onOpenSettings, historyOpen, onToggleHistory, menuItems }: Props): ReactElement {
  const requestedPresence = presenceView(snapshot, watchChanging);
  const [presence, setPresence] = useState<StablePresence>(() => ({ view: requestedPresence }));
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    setPresence((current) => stabilizePresence(current, requestedPresence, Date.now()));
  }, [requestedPresence.mode, requestedPresence.text]);
  useEffect(() => {
    const candidate = presence.candidate;
    if (candidate === undefined) return;
    const timer = window.setTimeout(() => {
      setPresence((current) => stabilizePresence(current, candidate.view, Date.now()));
    }, Math.max(0, 2_000 - (Date.now() - candidate.since)));
    return () => window.clearTimeout(timer);
  }, [presence.candidate?.since, presence.candidate?.view.mode, presence.candidate?.view.text]);
  useEffect(() => {
    if (!menuOpen) return;
    const closeOnPointerDown = (event: PointerEvent): void => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const closeOnKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeOnPointerDown, true);
    document.addEventListener("keydown", closeOnKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown, true);
      document.removeEventListener("keydown", closeOnKeyDown, true);
    };
  }, [menuOpen]);
  const selectMenuItem = (item: HeaderMenuItem): void => {
    setMenuOpen(false);
    item.onSelect();
  };
  const color = avatarColor(snapshot.config.ui.avatarColor);
  const cooAwake = watchIntentActive || audioEnabled;
  return <header className="presence-header">
    <div className="presence-identity">
      <div className={`presence-avatar presence-${presence.view.mode}`}>
        <AvatarBlob color={color} image={snapshot.avatarImagePng} size={40} state={cooAwake ? "open" : "resting"} squashed={!cooAwake} animated />
      </div>
      <div className="presence-controls">
        <button
          className={`watch-switch${watchIntentActive ? " is-on" : ""}${watchChanging ? " is-changing" : ""}`}
          type="button"
          role="switch"
          aria-checked={watchIntentActive}
          aria-label="Vision AI"
          title="Vision AI"
          onClick={onToggleWatch}
        >
          <span>Vision</span>
        </button>
        {onToggleAudio === undefined ? null : <button
          className={`watch-switch hearing-switch${audioEnabled ? " is-on" : ""}${audioChanging ? " is-changing" : ""}`}
          type="button"
          role="switch"
          aria-checked={audioEnabled}
          aria-label="Hearing AI"
          title="Hearing AI"
          onClick={onToggleAudio}
        >
          <span>Hearing</span>
        </button>}
      </div>
    </div>
    <div className="presence-actions">
      <button className={`history-toggle${historyOpen ? " is-active" : ""}`} type="button" aria-pressed={historyOpen} onClick={onToggleHistory}>履歴</button>
      <div ref={menuRef}>
        <HeaderMenu
          items={menuItems}
          open={menuOpen}
          onToggle={() => setMenuOpen((current) => !current)}
          onSelect={selectMenuItem}
        />
      </div>
      <button className="icon-button" type="button" aria-label="設定を開く" title="設定" onClick={onOpenSettings}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="3" /><path d="M19.2 13.8a7.8 7.8 0 0 0 0-3.6l2-1.5-2-3.4-2.5 1a8 8 0 0 0-3.1-1.8L13.2 2H9.3l-.4 2.5a8 8 0 0 0-3.1 1.8l-2.4-1-2 3.4 2 1.5a7.8 7.8 0 0 0 0 3.6l-2 1.5 2 3.4 2.4-1a8 8 0 0 0 3.1 1.8l.4 2.5h3.9l.4-2.5a8 8 0 0 0 3.1-1.8l2.5 1 2-3.4-2-1.5Z" /></svg>
      </button>
    </div>
  </header>;
}
