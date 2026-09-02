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
import { diag, verbose } from "./diag";
import { Game, PunktfunkMark, streamFrom } from "./game";
import {
  getHostStore,
  HostView,
  refreshHostsIfStale,
  useHostStore,
} from "./hooks";
import { collectElements, createRenderPatcher, describe } from "./patch";
import { hasPlayFromSelection, patchPlayGroup, resetPlayFrom, subscribePlayFrom } from "./play-from";
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
  /* Steam's own Play button while a Punktfunk host is chosen in its dropdown: Steam's green is a
     sliding gradient, so ours is the same gradient in violet. */
  .punktfunk-play {
    background: linear-gradient(to right, #8c7ef5 0%, #5b4ce0 60%) 25% center / 330% 100% !important;
    color: #ffffff !important;
  }
  .punktfunk-play:hover,
  .punktfunk-play.gpfocus,
  .punktfunk-play:focus {
    background-position: 0% center !important;
  }
`;

/**
 * The button is always on a Steam title's page, as a status as much as a control: violet when a
 * paired host can stream the title, gray when none can. Several hosts add a chevron segment with
 * Steam's own menu; the main segment streams from the best host. `inFlow` is the normal case —
 * spliced into the play bar's own button group, so it lays out and navigates as one of them;
 * without it the button floats from the anchor (the fallback when that group cannot be found).
 */
const StreamButton: FC<Game & { inFlow?: boolean }> = ({ inFlow, ...game }) => {
  const hosts = useHostsForApp(game.appId);
  const streaming = useGameStreaming(game.appId);
  const [, bumpSelection] = useState(0);
  useEffect(() => subscribePlayFrom(() => bumpSelection((n) => n + 1)), []);
  if (hasPlayFromSelection(game.appId, hosts)) {
    // Steam's own Play button is the violet Stream while a host is chosen in its dropdown
    // (see play-from.tsx); a second one beside the ⚙ would say the same thing twice.
    return null;
  }
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
  const row = (
    <>
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
    </>
  );
  if (inFlow) {
    return row;
  }
  return (
    <Focusable
      className={joinClassNames(basicAppDetailsSectionStylerClasses.AppButtons, "punktfunk-stream")}
    >
      {row}
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

// ---- Into the play bar itself --------------------------------------------------------------
//
// Measured on a Deck: from the InnerContainer, the play section is a Focusable whose child
// (props `overview` + `onGameInfoButtonToggle`) renders a chain of "section" components (props
// `setSections` + `overview`), several of which come in identical-looking pairs, until one
// renders the play bar row: [Play group, stats, button group]. The button group is the
// Focusable with the AppButtons class holding the controller and settings buttons. Landing
// INSIDE that group is what makes the button lay out and navigate as one of Steam's own.
//
// Every section-shaped child is patched (one wrap per component type, cached by Decky's
// patcher) and the shared handler either splices the button into the group it finds in its
// output or keeps descending. A branch that never reaches the group costs one tree walk.

/** The title of the page being rendered; set by the route handler before the descent runs. */
let currentGame: Game | null = null;
let placedInFlow = false;
let deepFailed = false;
let deepTimer: ReturnType<typeof setTimeout> | null = null;

const isPlaySection = (x: any): boolean => !!x?.props && "onGameInfoButtonToggle" in x.props && "overview" in x.props;
const isSectionShaped = (x: any): boolean => !!x?.props && "setSections" in x.props && "overview" in x.props;

/** Splice the button into the play bar's button group if `out` contains it; otherwise patch
 *  every section-shaped child so their renders come back through here. Never throws: a miss
 *  leaves Steam's output untouched, because an exception here is Steam's error screen. */
function onSectionRender(out: any): any {
    if (!out || !currentGame) {
      return out;
    }
    const group = findInReactTree(
      out,
      (x: any) =>
        Array.isArray(x?.props?.children) &&
        typeof x?.props?.className === "string" &&
        x.props.className.includes(basicAppDetailsSectionStylerClasses.AppButtons),
    ) as InnerContainer | undefined;
    if (group) {
      const kids = group.props.children as any[];
      if (!kids.some((c) => c?.key === ANCHOR_KEY)) {
        kids.splice(0, 0, <StreamButton key={ANCHOR_KEY} inFlow {...currentGame} />);
      }
      if (verbose()) {
        diag(`deep: group found in ${describe(out)} → ${kids.length} kids, frozen=${Object.isFrozen(kids)}`);
      }
      if (!placedInFlow) {
        placedInFlow = true;
        deepFailed = false; // the in-flow button is showing; the anchor must not join it
        diag(`game page ${currentGame.appId}: button placed in the play bar's button group`);
      }
      // The same row holds the Play group: its ▾ menu and Play button get Punktfunk's hosts.
      patchPlayGroup(out);
      return out;
    }
    const next = collectElements(out, isSectionShaped);
    if (verbose()) {
      diag(`deep: ${describe(out)} → ${next.length} section child(ren): ${next.map(describe).join(", ")}`);
    }
    if (next.length === 0 && !placedInFlow) {
      // A dead end: say what this output's Focusables are called, so a moved class is visible.
      const classes = collectElements(out, (x: any) => typeof x?.props?.className === "string")
        .map((x: any) => String(x.props.className).split(" ")[0])
        .slice(0, 6);
      diag(`deep: no group in [${classes.join(" ")}] (want ${basicAppDetailsSectionStylerClasses.AppButtons})`);
    }
    for (const child of next) {
      sections.patch(child);
    }
    return out;
}

const sections = createRenderPatcher(onSectionRender, "sections");

/** Forget the wrappers — for dismount. Steam's originals were never touched. */
function unpatchSections(): void {
  sections.reset();
  resetPlayFrom();
  placedInFlow = false;
}

/** Start the descent from the InnerContainer's render. Returns false when the play section
 *  itself is missing — then the anchor fallback is the only option this render. */
function placeInPlayBar(ret: ReactElement, game: Game): boolean {
  const section = findInReactTree(ret, isPlaySection);
  if (!section) {
    return false;
  }
  currentGame = game;
  sections.patch(section);
  if (!placedInFlow && !deepTimer) {
    // The group only shows up once the children have rendered. If it has not within a few
    // seconds of the first attempt, Steam's tree has moved: fall back to the floating anchor.
    deepTimer = setTimeout(() => {
      if (!placedInFlow) {
        deepFailed = true;
        diag("game page: play-bar group not found — using the anchor fallback");
      }
    }, 4000);
  }
  return true;
}

/**
 * Patch the game page: find the route's render function, and after each render splice the
 * anchor in just before the play panel (the child that carries the app overview and the launch
 * callback). Every lookup is defensive — Steam's tree is not an API, and a miss must leave the
 * page exactly as Steam drew it.
 */
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
        // Guarded: an exception in a route patch is Steam's error screen for the whole page.
        try {
          return placeOnPage(ret);
        } catch (e) {
          diag(`game page: handler failed: ${e}`);
          return ret;
        }
      },
      "punktfunk-stream",
    );
    afterPatch(routeProps, "renderFunc", handler);

    function placeOnPage(ret?: ReactElement): ReactElement | undefined {
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
        const game: Game = {
          appId,
          title: overview.display_name,
          iconHash: typeof overview.icon_hash === "string" ? overview.icon_hash : "",
        };
        // Normal path: into the play bar's own button group (see placeInPlayBar). The anchor
        // below is the fallback for a Steam tree where that group cannot be reached.
        if (placeInPlayBar(ret, game) && !(deepFailed && !placedInFlow)) {
          return ret;
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
        children.splice(at, 0, <StreamButtonAnchor key={ANCHOR_KEY} {...game} />);
        return ret;
    }
    return tree;
  });
}

/** Install the game-page patch. Returns the remover for `onDismount`. */
export function installGamePageStream(): () => void {
  const patch = patchLibraryApp();
  return () => {
    routerHook.removePatch(ROUTE, patch);
    unpatchSections();
  };
}
