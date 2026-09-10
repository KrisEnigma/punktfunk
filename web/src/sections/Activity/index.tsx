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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { eventKindLabel } from "@/lib/event-kinds";
import { fmtDateTime } from "@/lib/format";
import { m } from "@/paraglide/messages";

/** How many rows the dashboard card keeps. The rest are a click away, not gone. */
const CARD_MAX = 12;

/** Tighter than the house 0.1 s: a dozen short rows at 0.1 s took over a second to arrive. */
const ROW_GAP = 0.035;

/** How many rows still step. Past this they share the last one's delay, or a 200-entry ring on
 *  the full page would take seconds to finish arriving. */
const STAGGER_STEPS = 10;

/**
 * A row's arrival. Sliding from above is the direction the list grows: a new event is prepended,
 * so it comes in over the row it displaced.
 *
 * The delay lives on the ENTER transition, keyed by `custom`, and never on the row's own
 * `transition` — that one also drives the row's `layout` animation. Every arrival re-indexes the
 * survivors, so each slid down on its own 0–1 s delay: pure Y movement with no fade, and with
 * events a second apart the list never finished settling. Measured: rows mid-move in every frame.
 */
const ROW_VARIANTS = {
	from: { opacity: 0, y: -10 },
	enter: (i: number) => ({
		opacity: 1,
		y: 0,
		transition: { delay: Math.min(i, STAGGER_STEPS) * ROW_GAP },
	}),
	exit: { opacity: 0, y: 6 },
};

/**
 * The feed itself.
 *
 * `AnimatePresence` is what makes this readable while it is live: without it a row pushed past
 * the cap by a newer event simply vanished mid-glance, and `layout` carries the survivors down
 * rather than snapping them.
 *
 * Each row drives its own `from → enter` rather than inheriting it from a `<Stagger>` container.
 * Inherited, the rows never received a driving `animate` and landed on a single frame — and
 * removing `AnimatePresence` did not change that; giving each row its own `initial`/`animate` did.
 * A screenshot cannot tell a flattened list from a staggered one, which is why that took measuring.
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
					custom={i}
					className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2 first:pt-0 last:pb-0"
				>
					<Badge variant={toneFor(e.kind)}>{eventKindLabel(e.kind)}</Badge>
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
