// The Stream button on Steam's own game page.
//
// Steam Remote Play turns a title's Play button into "Stream" when another of your Steam
// clients has it installed — the page reads the app's per-client data and draws the primary
// button for the selected client. A plugin cannot join that list (the clients are Steam's,
// and the button is Steam's React tree), so this does what MoonDeck does: patch the
// `/library/app/:appid` route and drop a sibling button into the play bar, drawn like the
// bar's own buttons. It shows when a paired host's library has `steam:<appid>`; a tap streams
// that title from the host — the host launches it, the Deck shows it. Installed or not on the
// Deck makes no difference, exactly as with Steam Link.
import { RoutePatch, routerHook, toaster } from "@decky/api";
import {
  afterPatch,
  appDetailsClasses,
  appDetailsHeaderClasses,
  basicAppDetailsSectionStylerClasses,
  createReactTreePatcher,
  DialogButton,
  findInReactTree,
  Focusable,
  joinClassNames,
  Menu,
  MenuItem,
  playSectionClasses,
  showContextMenu,
} from "@decky/ui";
import { FC, ReactElement, useEffect, useRef, useState } from "react";
import { FaChevronDown, FaStop } from "react-icons/fa";
import { hostsForApp, subscribeCatalog } from "./catalog";
import {
  getHostStore,
  HostView,
  refreshHostsIfStale,
  startGameStream,
  useHostStore,
} from "./hooks";
import { isGameStreaming, stopGameStream, subscribeRunning } from "./steam";

const ROUTE = "/library/app/:appid";
const ANCHOR_KEY = "punktfunk-stream";

// On by default. The one preference the plugin keeps for itself: it is about the plugin's own
// footprint in Steam's UI, not about streaming, so it has no home in the client's settings.
const PREF_KEY = "punktfunk:gamePageStream";

export function gamePageStreamEnabled(): boolean {
  try {
    return localStorage.getItem(PREF_KEY) !== "0";
  } catch {
    return true;
  }
}

export function setGamePageStreamEnabled(on: boolean): void {
  try {
    localStorage.setItem(PREF_KEY, on ? "1" : "0");
  } catch {
    /* ignore */
  }
}

/** A page opened this long after the last scan rescans in the background; the button still
 *  draws from the cache meanwhile, so an open page never waits on the network. */
const STALE_MS = 60_000;

/** Steam's `app_type` for a non-Steam shortcut — never a host's `steam:<appid>`. */
const APP_TYPE_SHORTCUT = 1073741824;

/** The hosts that can stream `appId`, live: re-evaluated on every scan and library answer. */
function useHostsForApp(appId: number): HostView[] {
  const { views } = useHostStore();
  const [, bump] = useState(0);
  useEffect(() => subscribeCatalog(() => bump((n) => n + 1)), []);
  useEffect(() => {
    void refreshHostsIfStale(STALE_MS);
  }, [appId]);
  return hostsForApp(appId, views);
}

/** Is this title's stream up right now — live, from Steam's app lifetime feed. */
function useGameStreaming(appId: number): boolean {
  const [streaming, setStreaming] = useState(() => isGameStreaming(appId));
  useEffect(() => {
    setStreaming(isGameStreaming(appId));
    return subscribeRunning(() => setStreaming(isGameStreaming(appId)));
  }, [appId]);
  return streaming;
}

/** What the game page knows about its title; what the per-game shortcut is dressed with. */
interface Game {
  appId: number;
  title: string;
  iconHash: string;
}

function streamFrom(host: HostView, game: Game): void {
  // A sleeping host is the one case that takes a while and looks like nothing happened.
  if (!host.online) {
    toaster.toast({ title: "Punktfunk", body: `Waking ${host.name} to stream ${game.title}` });
  }
  void startGameStream(host, game.appId, game.title, game.iconHash);
}

/** Several hosts have the title: Steam's own context menu, the way its Play dropdown lists
 *  the clients a game could run on. Anchored to the chevron segment that opened it. */
function openHostMenu(hosts: HostView[], game: Game, anchor: EventTarget): void {
  showContextMenu(
    <Menu label={`Stream ${game.title} from`}>
      {hosts.map((h) => (
        <MenuItem key={h.ref} onSelected={() => streamFrom(h, game)}>
          {h.name}
          <span style={{ opacity: 0.6 }}> · {h.online ? "online" : "asleep, will wake"}</span>
        </MenuItem>
      ))}
    </Menu>,
    anchor,
  );
}

/** The Punktfunk lens mark (the two overlapping circles of the logo). Fills come from the
 *  button's CSS variables, so the mark follows the button's ready / idle / focused state. */
const PunktfunkMark: FC<{ ready: boolean }> = ({ ready }) => (
  <svg viewBox="17 13 141 141" width="22" height="22" aria-hidden="true">
    <defs>
      <linearGradient id="pf-lens" x1="0" y1="1" x2="1" y2="0">
        <stop offset="0" stopColor="#ffffff" stopOpacity="0" />
        <stop offset="1" stopColor="#ffffff" stopOpacity="0.9" />
      </linearGradient>
    </defs>
    <circle cx="65.44" cy="105.85" r="44.3" style={{ fill: "var(--pf-back)" }} />
    <circle cx="109.74" cy="61.55" r="44.3" style={{ fill: "var(--pf-deep)" }} />
    {ready && (
      <path
        fill="url(#pf-lens)"
        d="M121.228,104.359c-14.777,3.965 -31.187,0.136 -42.811,-11.488c-11.624,-11.624 -15.453,-28.034 -11.488,-42.811c14.777,-3.965 31.187,-0.136 42.811,11.488c11.624,11.624 15.453,28.034 11.488,42.811Z"
      />
    )}
  </svg>
);

// Anchored at the boundary between the header and the play section. Measured on a Deck: the
// bar's ⚙ / ℹ buttons are 48 px squares starting 16 px below that boundary, the ℹ ending at
// the 2.8vw page margin and the ⚙ 10 px to its left with a 10 px margin of its own — so the
// slot before them ends at 2.8vw + 116 px. MenuButton gives Steam's height, radius and focus
// behaviour; its fixed 48 px width is lifted so the label fits. z-index lifts the button above
// the play section, a later sibling that otherwise paints over it.
//
// Colour is the state, the way Play is green when it can be pressed: brand violet
// (assets/punktfunk-logo.svg) when a host can stream the title, Steam's muted gray when none
// can. Steam styles this element through `button.<hash>.DialogButton:enabled…` selectors and
// turns it white on focus; the ready state's colours are `!important` so they hold, and it
// brightens under focus instead, while the idle state keeps Steam's own white focus.
const STYLE = `
  .punktfunk-stream {
    position: absolute;
    right: calc(2.8vw + 116px);
    top: 16px;
    z-index: 1;
  }
  .punktfunk-stream-row {
    display: flex;
    align-items: stretch;
  }
  .punktfunk-stream-button,
  .punktfunk-stream-more {
    margin: 0 !important;
    width: auto !important;
    min-width: 0 !important;
    display: flex !important;
    align-items: center;
    white-space: nowrap;
    transition: background-color 0.2s, color 0.2s;
  }
  .punktfunk-stream-button {
    padding: 0 16px 0 12px !important;
    gap: 8px;
    font-weight: 500;
  }
  .punktfunk-stream-button.is-idle {
    color: #8b929a !important;
    --pf-back: #6b7079;
    --pf-deep: #8b929a;
  }
  .punktfunk-stream-button.is-idle:focus {
    --pf-back: #9aa0a8;
    --pf-deep: #5c6068;
  }
  .punktfunk-stream-button.is-ready,
  .punktfunk-stream-more.is-ready {
    background: #6c5bf3 !important;
    color: #ffffff !important;
    --pf-back: #cec9fb;
    --pf-deep: #f2f1fe;
  }
  .punktfunk-stream-more.is-ready {
    background: #5b4ce0 !important;
  }
  .punktfunk-stream-button.is-ready:hover,
  .punktfunk-stream-more.is-ready:hover {
    background: #7b6cf6 !important;
  }
  .punktfunk-stream-button.is-ready:focus,
  .punktfunk-stream-more.is-ready:focus,
  .punktfunk-stream-button.is-ready.gpfocus,
  .punktfunk-stream-more.is-ready.gpfocus {
    background: #8574f7 !important;
    color: #ffffff !important;
  }
  .punktfunk-stream-button.is-ready svg,
  .punktfunk-stream-more.is-ready svg {
    color: #ffffff !important;
  }
  .punktfunk-stream-row.has-more .punktfunk-stream-button {
    border-top-right-radius: 0 !important;
    border-bottom-right-radius: 0 !important;
  }
  .punktfunk-stream-more {
    padding: 0 10px !important;
    margin-inline-start: 0 !important;
    border-top-left-radius: 0 !important;
    border-bottom-left-radius: 0 !important;
    border-left: 1px solid rgba(255, 255, 255, 0.18);
    font-size: 0.7em;
  }
`;

/**
 * The button is always on a Steam title's page, as a status as much as a control: the lens mark
 * is violet when a paired host can stream the title, gray when none can. Several hosts add a
 * chevron segment with Steam's own menu; the main segment streams from the best host.
 */
const StreamButton: FC<Game> = (game) => {
  const hosts = useHostsForApp(game.appId);
  const streaming = useGameStreaming(game.appId);
  const ready = hosts.length > 0;
  const onClick = () => {
    if (streaming) {
      stopGameStream(game.appId); // Steam's Stop, for the stream this page started
    } else if (ready) {
      streamFrom(hosts[0], game); // best host: online first, then most recently used
    } else {
      toaster.toast({
        title: "Punktfunk",
        body: `No paired host has ${game.title} in its library`,
      });
    }
  };
  const hasMore = ready && !streaming && hosts.length > 1;
  return (
    <Focusable
      className={joinClassNames(basicAppDetailsSectionStylerClasses.AppButtons, "punktfunk-stream")}
    >
      <style>{STYLE}</style>
      <Focusable
        className={joinClassNames("punktfunk-stream-row", hasMore ? "has-more" : "")}
        flow-children="row"
      >
        <DialogButton
          className={joinClassNames(
            playSectionClasses.MenuButton,
            "punktfunk-stream-button",
            ready || streaming ? "is-ready" : "is-idle",
          )}
          onClick={onClick}
        >
          {streaming ? <FaStop /> : <PunktfunkMark ready={ready} />}
          {streaming ? "Stop" : "Stream"}
        </DialogButton>
        {hasMore && (
          <DialogButton
            className={joinClassNames(
              playSectionClasses.MenuButton,
              "punktfunk-stream-more",
              "is-ready",
            )}
            onClick={(e) => openHostMenu(hosts, game, e.currentTarget as EventTarget)}
          >
            <FaChevronDown />
          </DialogButton>
        )}
      </Focusable>
    </Focusable>
  );
};

/** The header element whose class flips while the hero art goes fullscreen (the page's
 *  "scroll up" state). The button must leave with the play bar, or it floats over the art. */
function findTopCapsule(anchor: HTMLDivElement | null): Element | null {
  const siblings = anchor?.parentElement?.children;
  if (!siblings) {
    return null;
  }
  for (const sibling of siblings) {
    if (!sibling.className.includes(appDetailsClasses.Header)) {
      continue;
    }
    for (const child of sibling.children) {
      if (child.className.includes(appDetailsHeaderClasses.TopCapsule)) {
        return child;
      }
    }
  }
  return null;
}

const StreamButtonAnchor: FC<Game> = (props) => {
  const [show, setShow] = useState(true);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const capsule = findTopCapsule(ref.current);
    if (!capsule) {
      return;
    }
    const observer = new MutationObserver((entries) => {
      for (const entry of entries) {
        if (entry.type !== "attributes" || entry.attributeName !== "class") {
          continue;
        }
        const cls = (entry.target as Element).className;
        const fullscreen =
          cls.includes(appDetailsHeaderClasses.FullscreenEnterStart) ||
          cls.includes(appDetailsHeaderClasses.FullscreenEnterActive) ||
          cls.includes(appDetailsHeaderClasses.FullscreenEnterDone) ||
          cls.includes(appDetailsHeaderClasses.FullscreenExitStart) ||
          cls.includes(appDetailsHeaderClasses.FullscreenExitActive);
        const aborted = cls.includes(appDetailsHeaderClasses.FullscreenExitDone);
        setShow(!fullscreen || aborted);
      }
    });
    observer.observe(capsule, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  return (
    <div id="punktfunk-stream-anchor" ref={ref} style={{ position: "relative", height: 0 }}>
      {show && <StreamButton {...props} />}
    </div>
  );
};

interface OverviewLike {
  appid?: unknown;
  app_type?: unknown;
  display_name?: unknown;
  icon_hash?: unknown;
}

type PanelChild = ReactElement<{
  overview?: unknown;
  onShowLaunchingDetails?: unknown;
  fullscreen?: unknown;
}>;
type InnerContainer = ReactElement<{ children: PanelChild[]; className?: string }>;

/**
 * Patch the game page: find the route's render function, and after each render splice the
 * anchor in just before the play panel (the child that carries the app overview and the launch
 * callback). Every lookup is defensive — Steam's tree is not an API, and a miss must leave the
 * page exactly as Steam drew it.
 */
// Where each patch attempt got to, readable from the CEF debugger as `window.__punktfunkDiag`
// and echoed to the console. Steam's tree is not an API; when it moves, this says which step.
declare global {
  interface Window {
    __punktfunkDiag?: string[];
  }
}
let lastDiag = "";
function diag(msg: string): void {
  if (msg === lastDiag) {
    return; // the page renders several times per open; one line per change is enough
  }
  lastDiag = msg;
  const line = `${new Date().toISOString().slice(11, 19)} ${msg}`;
  console.warn(`punktfunk: ${msg}`);
  try {
    (window.__punktfunkDiag ??= []).push(line);
    if (window.__punktfunkDiag.length > 60) {
      window.__punktfunkDiag.shift();
    }
  } catch {
    /* ignore */
  }
}

function patchLibraryApp(): RoutePatch {
  return routerHook.addPatch(ROUTE, (tree: any) => {
    const routeProps = findInReactTree(tree, (x: any) => x?.renderFunc);
    if (!routeProps) {
      diag("game page: no renderFunc in route tree");
      return tree;
    }
    let overview: OverviewLike | undefined;
    const handler = createReactTreePatcher(
      [
        (node: any) => {
          const children = findInReactTree(node, (x: any) => x?.props?.children?.props?.overview)
            ?.props?.children;
          if (typeof children !== "object" || typeof children?.props?.overview !== "object") {
            diag("game page: no child carrying an overview");
            return null;
          }
          overview = children.props.overview as OverviewLike;
          return children;
        },
      ],
      (_: unknown[], ret?: ReactElement) => {
        if (!ret || !gamePageStreamEnabled()) {
          diag(`game page: ${ret ? "button disabled by preference" : "empty render"}`);
          return ret;
        }
        const appId = overview?.appid;
        if (
          typeof appId !== "number" ||
          overview?.app_type === APP_TYPE_SHORTCUT ||
          typeof overview?.display_name !== "string"
        ) {
          diag(`game page: skipped overview appid=${String(appId)} type=${String(overview?.app_type)}`);
          return ret;
        }
        const parent = findInReactTree(
          ret,
          (x: InnerContainer) =>
            Array.isArray(x?.props?.children) &&
            !!x?.props?.className?.includes(appDetailsClasses.InnerContainer),
        ) as InnerContainer | undefined;
        if (!parent) {
          diag(`game page ${appId}: no InnerContainer (${appDetailsClasses.InnerContainer})`);
          return ret;
        }
        const children = parent.props.children;
        if (children.some((c) => c?.key === ANCHOR_KEY)) {
          return ret; // already spliced into this render's tree
        }
        // The children are [header, play section, launching-details] — the last carries the
        // overview and the launch callback but draws nothing, so it is only a witness that this
        // is the page. The anchor goes right AFTER THE HEADER (the child with the `fullscreen`
        // flag), which is the boundary the play bar hangs from.
        const panelIndex = children.findIndex(
          (c) => c?.props?.overview && c?.props?.onShowLaunchingDetails,
        );
        if (panelIndex < 0) {
          diag(
            `game page ${appId}: no play panel among ${children.length} children: ` +
              children.map((c) => Object.keys(c?.props ?? {}).join("+") || String(c)).join(" | "),
          );
          return ret;
        }
        const headerIndex = children.findIndex(
          (c) => c?.props?.overview && "fullscreen" in (c.props as object),
        );
        const at = headerIndex >= 0 ? headerIndex + 1 : Math.max(panelIndex - 1, 0);
        diag(
          `game page ${appId}: anchor spliced at ${at}/${children.length}, ` +
            `${hostsForApp(appId, getHostStore().views).length} host(s) have it`,
        );
        children.splice(
          at,
          0,
          <StreamButtonAnchor
            key={ANCHOR_KEY}
            appId={appId}
            title={overview.display_name}
            iconHash={typeof overview.icon_hash === "string" ? overview.icon_hash : ""}
          />,
        );
        return ret;
      },
      "punktfunk-stream",
    );
    afterPatch(routeProps, "renderFunc", handler);
    return tree;
  });
}

/** Install the game-page patch. Returns the remover for `onDismount`. */
export function installGamePageStream(): () => void {
  const patch = patchLibraryApp();
  return () => {
    routerHook.removePatch(ROUTE, patch);
  };
}
