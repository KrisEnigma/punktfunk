// What this host has been doing — the event stream, rendered.
//
// The console could describe the present (a status snapshot) but never the recent past: a client
// that connected and left while you were on another page left no trace anywhere you could look.
// The stream was already open for cache invalidation, so this costs one ring buffer.
//
// In-memory and bounded, so it starts empty on a page load and fills as things happen. That is the
// honest shape for a live tail — pretending to be a durable log would need the host to keep one.
//
// Two surfaces, one list: the dashboard card shows the newest handful, and `/activity` shows the
// whole ring. The card used to render all 200, which pushed everything below it off the page on a
// busy host.

import { Link } from "@tanstack/react-router";
import Section from "@unom/ui/section";
import { Activity as ActivityIcon, ArrowRight } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { FC } from "react";
import { type ActivityEntry, useActivity } from "@/api/events";
import { STAGGER_GAP } from "@/components/stagger";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { fmtDateTime } from "@/lib/format";
import { m } from "@/paraglide/messages";

/** How many rows the dashboard card keeps. The rest are a click away, not gone. */
const CARD_MAX = 12;

/**
 * A row's arrival. Sliding from above is the direction the list actually grows: a new event is
 * prepended, so it comes in over the row it displaced.
 */
const ROW_VARIANTS = {
	from: { opacity: 0, y: -10 },
	enter: { opacity: 1, y: 0 },
	exit: { opacity: 0, y: 6 },
};

/** How many rows still step. Past this they share the last one's delay: at the house 0.1 s a
 *  200-entry ring would otherwise take twenty seconds to finish arriving. */
const STAGGER_STEPS = 10;

/**
 * The feed itself.
 *
 * `AnimatePresence` is what makes this readable while it is live: without it a row pushed past
 * the cap by a newer event simply vanished mid-glance, and `layout` carries the survivors down
 * rather than snapping them.
 *
 * The cadence is a per-row `delay` and NOT the house `<Stagger>` container, which is the one
 * surprise here. A stagger container works by propagating the variant NAME down and orchestrating
 * the children itself; `AnimatePresence` sits between the two and the children never inherit, so
 * the rows arrived on a single frame with every prop looking correct. Measured, not guessed — a
 * screenshot cannot tell the two apart. Each row therefore drives its own `from → enter` and
 * spaces itself by index.
 *
 * That indexing also does the right thing while live: a new event mounts at index 0, so it lands
 * immediately, while a freshly loaded page fills in on the cadence.
 *
 * NOT `initial={false}`: that is the "these were already here" switch, and it would skip the
 * entrance for exactly the batch worth animating.
 */
export const ActivityList: FC<{ entries: ActivityEntry[] }> = ({ entries }) => (
	<ul className="flex flex-col divide-y">
		<AnimatePresence>
			{entries.map((e, i) => (
				<motion.li
					key={e.seq}
					layout
					initial="from"
					animate="enter"
					exit="exit"
					variants={ROW_VARIANTS}
					transition={{
						delay: Math.min(i, STAGGER_STEPS) * STAGGER_GAP,
					}}
					className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2 first:pt-0 last:pb-0"
				>
					<Badge variant={toneFor(e.kind)}>{kindLabel(e.kind)}</Badge>
					<span className="min-w-0 flex-1 truncate text-sm">{describe(e)}</span>
					<time
						dateTime={new Date(e.ts_ms).toISOString()}
						className="shrink-0 text-xs tabular-nums text-muted-foreground"
					>
						{fmtDateTime(e.ts_ms)}
					</time>
				</motion.li>
			))}
		</AnimatePresence>
	</ul>
);

/** The dashboard's card: the newest few, with the way to the rest. */
export const ActivityCard: FC = () => (
	<ActivityCardView entries={useActivity()} />
);

/** Split from the hook so a story can hand it a busy host — the cap and the cadence are the
 *  parts worth looking at, and neither shows up without one. */
export const ActivityCardView: FC<{ entries: ActivityEntry[] }> = ({
	entries,
}) => {
	return (
		<Card>
			<CardHeader>
				<CardTitle className="flex items-center gap-2">
					<ActivityIcon className="size-4" />
					{m.activity_title()}
				</CardTitle>
			</CardHeader>
			<CardContent className="space-y-3">
				{entries.length === 0 ? (
					<p className="text-sm text-muted-foreground">{m.activity_empty()}</p>
				) : (
					<>
						<ActivityList entries={entries.slice(0, CARD_MAX)} />
						<div className="flex justify-end border-t pt-3">
							<Button asChild variant="ghost" size="sm">
								<Link to="/activity">
									{m.activity_show_all()}
									<ArrowRight className="size-4" />
								</Link>
							</Button>
						</div>
					</>
				)}
			</CardContent>
		</Card>
	);
};

/** `/activity`: the whole ring, which is everything since this page was loaded. */
export const SectionActivity: FC = () => (
	<ActivityPage entries={useActivity()} />
);

export const ActivityPage: FC<{ entries: ActivityEntry[] }> = ({ entries }) => {
	return (
		<Section maxWidth={false}>
			<div className="flex flex-col gap-card">
				<h1 className="text-2xl font-semibold">{m.activity_title()}</h1>
				<Card>
					<CardContent>
						{entries.length === 0 ? (
							<p className="text-sm text-muted-foreground">
								{m.activity_empty()}
							</p>
						) : (
							<ActivityList entries={entries} />
						)}
					</CardContent>
				</Card>
				{/* The ring is per page load, and a reader who scrolled to the bottom of it has
				    earned that fact rather than wondering where last week went. */}
				<p className="text-xs text-muted-foreground">
					{m.activity_ring_note()}
				</p>
			</div>
		</Section>
	);
};

/** The subject of an event, in one line — whatever the payload actually names. */
function describe(e: ActivityEntry): string {
	const d = e.data;
	const client = pick(d.client, "name") ?? pick(d.session, "client");
	const stream = d.stream as Record<string, unknown> | undefined;
	const parts = [
		client,
		typeof stream?.app === "string" ? stream.app : undefined,
		typeof d.reason === "string" ? d.reason : undefined,
		typeof d.game === "string" ? d.game : undefined,
	].filter((x): x is string => typeof x === "string" && x.length > 0);
	// An event whose payload names nothing (host.started, library.changed) is still worth a row —
	// the kind badge carries the whole meaning, so leave the line blank rather than inventing text.
	return parts.join(" · ");
}

/** Read a string field off a nested ref object, tolerating anything unexpected. */
function pick(obj: unknown, key: string): string | undefined {
	if (!obj || typeof obj !== "object") return undefined;
	const v = (obj as Record<string, unknown>)[key];
	if (typeof v === "string") return v;
	// `SessionRef.client` is itself a ClientRef.
	if (v && typeof v === "object") {
		const name = (v as Record<string, unknown>).name;
		return typeof name === "string" ? name : undefined;
	}
	return undefined;
}

/** Colour by what the event means, not by its domain — good news green, losses muted, denials red. */
function toneFor(
	kind: string,
): "success" | "destructive" | "secondary" | "outline" {
	if (kind === "pairing.denied") return "destructive";
	if (kind.endsWith(".connected") || kind.endsWith(".started"))
		return "success";
	if (kind === "pairing.completed") return "success";
	if (kind.endsWith(".disconnected") || kind.endsWith(".ended"))
		return "outline";
	if (kind.endsWith(".stopped") || kind.endsWith(".exited")) return "outline";
	return "secondary";
}

/** Translated label per kind, falling back to the raw kind so a new host event still shows. */
const KIND_LABEL: Record<string, () => string> = {
	"client.connected": () => m.activity_client_connected(),
	"client.disconnected": () => m.activity_client_disconnected(),
	"session.started": () => m.activity_session_started(),
	"session.ended": () => m.activity_session_ended(),
	"stream.started": () => m.activity_stream_started(),
	"stream.stopped": () => m.activity_stream_stopped(),
	"game.running": () => m.activity_game_running(),
	"game.exited": () => m.activity_game_exited(),
	"pairing.pending": () => m.activity_pairing_pending(),
	"pairing.completed": () => m.activity_pairing_completed(),
	"pairing.denied": () => m.activity_pairing_denied(),
	"display.created": () => m.activity_display_created(),
	"display.released": () => m.activity_display_released(),
	"library.changed": () => m.activity_library_changed(),
	"update.available": () => m.activity_update_available(),
	"update.applied": () => m.activity_update_applied(),
	"plugins.changed": () => m.activity_plugins_changed(),
	"store.changed": () => m.activity_store_changed(),
	"host.started": () => m.activity_host_started(),
	"host.stopping": () => m.activity_host_stopping(),
};

function kindLabel(kind: string): string {
	return KIND_LABEL[kind]?.() ?? kind;
}
