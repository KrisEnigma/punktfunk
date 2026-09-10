import type { Meta, StoryObj } from "@storybook/react-vite";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { getGetLibraryQueryKey } from "@/api/gen/library/library";
import type { GameEntry } from "@/api/gen/model/gameEntry";
import type { NativeClient } from "@/api/gen/model/nativeClient";
import { getListNativeClientsQueryKey } from "@/api/gen/native/native";
import { HookForm } from "@/sections/Automation/HookForm";
import { nativeClients } from "./lib/fixtures";

/**
 * Adding an automation (`HookForm`).
 *
 * The two filter fields used to be bare text inputs: the operator had to already know a device's
 * exact label and a game's store-qualified id (`steam:570`) and type them from memory. They offer
 * what the host knows now — and stay free text, because a hook may name a game that is not
 * installed yet.
 *
 * `HugeLibrary` is the one that earns its keep. Ten thousand titles is a real Steam account, and
 * the field has to stay instant: the matches are filtered and capped before they reach the DOM,
 * so the option count stays bounded no matter how big the library is. An `<option>` per title
 * would put 10,000 nodes on the page for a field most people never open.
 */
const game = (i: number): GameEntry => ({
	art: {},
	id: `steam:${100000 + i}`,
	store: "steam",
	title: `${TITLES[i % TITLES.length]} ${Math.floor(i / TITLES.length) + 1}`,
});

const TITLES = [
	"Hades",
	"Celeste",
	"Hollow Knight",
	"Dota",
	"Factorio",
	"Stardew Valley",
	"Portal",
	"Terraria",
];

const HUGE: GameEntry[] = Array.from({ length: 10_000 }, (_, i) => game(i));
const SMALL: GameEntry[] = HUGE.slice(0, 12);

/** Seed the cache the form reads, so the story needs no host. */
function Seeded({
	library,
	children,
}: {
	library: GameEntry[];
	children: React.ReactNode;
}) {
	const qc = useQueryClient();
	useState(() => {
		qc.setQueryData(getGetLibraryQueryKey(), library);
		qc.setQueryData(
			getListNativeClientsQueryKey(),
			nativeClients as NativeClient[],
		);
		return null;
	});
	return <>{children}</>;
}

function Harness({ library }: { library: GameEntry[] }) {
	const [value, setValue] = useState<{
		on: string;
		run?: string | null;
		filter?: { client?: string | null; app?: string | null };
	} | null>({ on: "stream.started", run: "", filter: { app: "" } });
	return (
		<Seeded library={library}>
			<HookForm
				value={value}
				onCancel={() => setValue(null)}
				onSave={() => setValue(null)}
			/>
		</Seeded>
	);
}

const meta = {
	title: "Console/Automation",
	parameters: { layout: "padded" },
} satisfies Meta;
export default meta;

type Story = StoryObj<typeof meta>;

/** A handful of games, the ordinary case. */
export const AddHook: Story = { render: () => <Harness library={SMALL} /> };

/** Ten thousand of them. The suggestion list must stay bounded. */
export const HugeLibrary: Story = { render: () => <Harness library={HUGE} /> };
