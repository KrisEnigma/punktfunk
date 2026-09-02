// What the game page knows about its title, and the brand pieces drawn for it.
import { toaster } from "@decky/api";
import { FC } from "react";
import { HostView, startGameStream } from "./hooks";

/** What the game page knows about its title; what the per-game shortcut is dressed with. */
export interface Game {
  appId: number;
  title: string;
  iconHash: string;
}

export function streamFrom(host: HostView, game: Game): void {
  // A sleeping host is the one case that takes a while and looks like nothing happened.
  if (!host.online) {
    toaster.toast({ title: "Punktfunk", body: `Waking ${host.name} to stream ${game.title}` });
  }
  void startGameStream(host, game.appId, game.title, game.iconHash);
}

// Brand violets from assets/punktfunk-logo.svg.
export const VIOLET = "#6c5bf3";
export const VIOLET_LIGHT = "#a79ff8";

/** The Punktfunk lens mark (the two overlapping circles of the logo). Fills default to the
 *  enclosing button's CSS variables so the mark follows its ready / idle / focused state; pass
 *  `back` / `deep` for a fixed colouring. */
export const PunktfunkMark: FC<{ ready: boolean; size?: number; back?: string; deep?: string }> = ({
  ready,
  size = 22,
  back = "var(--pf-back)",
  deep = "var(--pf-deep)",
}) => (
  <svg viewBox="17 13 141 141" width={size} height={size} aria-hidden="true">
    <defs>
      <linearGradient id="pf-lens" x1="0" y1="1" x2="1" y2="0">
        <stop offset="0" stopColor="#ffffff" stopOpacity="0" />
        <stop offset="1" stopColor="#ffffff" stopOpacity="0.9" />
      </linearGradient>
    </defs>
    <circle cx="65.44" cy="105.85" r="44.3" style={{ fill: back }} />
    <circle cx="109.74" cy="61.55" r="44.3" style={{ fill: deep }} />
    {ready && (
      <path
        fill="url(#pf-lens)"
        d="M121.228,104.359c-14.777,3.965 -31.187,0.136 -42.811,-11.488c-11.624,-11.624 -15.453,-28.034 -11.488,-42.811c14.777,-3.965 31.187,-0.136 42.811,11.488c11.624,11.624 15.453,28.034 11.488,42.811Z"
      />
    )}
  </svg>
);
