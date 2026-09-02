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
  ModalRoot,
  playSectionClasses,
  showModal,
} from "@decky/ui";
import { FC, ReactElement, useEffect, useRef, useState } from "react";
import { FaPlay } from "react-icons/fa";
import { hostsForApp, subscribeCatalog } from "./catalog";
import { HostView, refreshHostsIfStale, startStream, useHostStore } from "./hooks";

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

function streamFrom(host: HostView, appId: number, title: string): void {
  // Steam's launch UI is about to say "Punktfunk", not the game — say what is really starting.
  toaster.toast({
    title: "Punktfunk",
    body: host.online
      ? `Streaming ${title} from ${host.name}`
      : `Waking ${host.name} to stream ${title}`,
  });
  void startStream(host, { gameId: `steam:${appId}` }, title);
}

/** More than one host has the title: the same choice Steam's own client dropdown offers. */
const HostPicker: FC<{
  hosts: HostView[];
  appId: number;
  title: string;
  closeModal?: () => void;
}> = ({ hosts, appId, title, closeModal }) => (
  <ModalRoot closeModal={closeModal}>
    <div style={{ fontWeight: "bold", fontSize: "1.3em", marginBottom: "0.3em" }}>
      Stream {title} from…
    </div>
    <Focusable style={{ display: "flex", flexDirection: "column", gap: "0.5em" }}>
      {hosts.map((h) => (
        <DialogButton
          key={h.ref}
          onClick={() => {
            closeModal?.();
            streamFrom(h, appId, title);
          }}
        >
          {h.name}
          <span style={{ opacity: 0.7 }}> · {h.online ? "online" : "asleep, will wake"}</span>
        </DialogButton>
      ))}
      <DialogButton onClick={() => closeModal?.()}>Cancel</DialogButton>
    </Focusable>
  </ModalRoot>
);

// Anchored at the boundary between the header and the play panel, so the bar's own bottom
// edge is the reference: in the play bar's right-hand group, at the ⚙ / ℹ buttons' height —
// the offsets MoonDeck ships as its default. Drawn with the bar's MenuButton class so the
// height, radius and focus ring are Steam's; only the label is ours.
const STYLE = `
  .punktfunk-stream {
    position: absolute;
    right: 2.8vw;
    bottom: 16px;
  }
  .punktfunk-stream-button {
    margin: 0 !important;
    min-width: 0 !important;
    padding: 0 18px !important;
    display: flex !important;
    align-items: center;
    white-space: nowrap;
    background: rgba(14, 20, 27, 0.5);
  }
  .punktfunk-stream-button:hover {
    background: rgba(14, 20, 27, 0.75);
  }
`;

const StreamButton: FC<{ appId: number; title: string }> = ({ appId, title }) => {
  const hosts = useHostsForApp(appId);
  if (hosts.length === 0) {
    return null;
  }
  const onClick = () => {
    if (hosts.length === 1) {
      streamFrom(hosts[0], appId, title);
    } else {
      showModal(<HostPicker hosts={hosts} appId={appId} title={title} />);
    }
  };
  return (
    <Focusable
      className={joinClassNames(basicAppDetailsSectionStylerClasses.AppButtons, "punktfunk-stream")}
    >
      <style>{STYLE}</style>
      <Focusable>
        <DialogButton
          className={joinClassNames(playSectionClasses.MenuButton, "punktfunk-stream-button")}
          onClick={onClick}
        >
          <FaPlay style={{ marginRight: "0.5em" }} />
          Stream
        </DialogButton>
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

const StreamButtonAnchor: FC<{ appId: number; title: string }> = (props) => {
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
}

type PanelChild = ReactElement<{ overview?: unknown; onShowLaunchingDetails?: unknown }>;
type InnerContainer = ReactElement<{ children: PanelChild[]; className?: string }>;

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
      return tree;
    }
    let overview: OverviewLike | undefined;
    const handler = createReactTreePatcher(
      [
        (node: any) => {
          const children = findInReactTree(node, (x: any) => x?.props?.children?.props?.overview)
            ?.props?.children;
          if (typeof children !== "object" || typeof children?.props?.overview !== "object") {
            return null;
          }
          overview = children.props.overview as OverviewLike;
          return children;
        },
      ],
      (_: unknown[], ret?: ReactElement) => {
        if (!ret || !gamePageStreamEnabled()) {
          return ret;
        }
        const appId = overview?.appid;
        if (
          typeof appId !== "number" ||
          overview?.app_type === APP_TYPE_SHORTCUT ||
          typeof overview?.display_name !== "string"
        ) {
          return ret;
        }
        const parent = findInReactTree(
          ret,
          (x: InnerContainer) =>
            Array.isArray(x?.props?.children) &&
            !!x?.props?.className?.includes(appDetailsClasses.InnerContainer),
        ) as InnerContainer | undefined;
        if (!parent) {
          return ret;
        }
        const children = parent.props.children;
        if (children.some((c) => c?.key === ANCHOR_KEY)) {
          return ret; // already spliced into this render's tree
        }
        const panelIndex = children.findIndex(
          (c) => c?.props?.overview && c?.props?.onShowLaunchingDetails,
        );
        if (panelIndex < 0) {
          return ret;
        }
        children.splice(
          panelIndex,
          0,
          <StreamButtonAnchor key={ANCHOR_KEY} appId={appId} title={overview.display_name} />,
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
