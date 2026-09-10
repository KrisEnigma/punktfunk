import { useId, useMemo } from "react";
import { Input } from "@/components/ui/input";

/** One thing worth offering: the value that gets stored, and the name a person recognises. */
export interface Suggestion {
	/** What lands in the field — an app id, a device name. */
	value: string;
	/** What the operator reads. Omitted when the value IS the readable thing. */
	label?: string;
}

/**
 * A text field that offers what it knows, without insisting on it.
 *
 * `<datalist>`, not a custom combobox, because the field has to stay FREE TEXT: a hook may filter
 * on a game that is not installed yet or a device that has not paired, and a widget that only
 * yields values from a list would make those unwritable. Suggestions are the whole contract, which
 * is what a datalist is — and it brings the browser's own popup, keyboard handling and screen
 * reader support rather than a reimplementation of all three.
 *
 * **Scale.** A library can hold tens of thousands of titles, so the matches are filtered here and
 * capped before they reach the DOM: rendering an <option> per title would put 10,000 nodes on the
 * page for a field most people never open. Filtering that many strings costs well under a frame;
 * rendering them does not.
 */
export function Suggest({
	value,
	onChange,
	suggestions,
	max = 50,
	...props
}: {
	value: string;
	onChange: (value: string) => void;
	suggestions: Suggestion[];
	/** How many options reach the DOM. */
	max?: number;
} & Omit<
	React.ComponentProps<typeof Input>,
	"value" | "onChange" | "list" | "type"
>) {
	const listId = useId();
	const shown = useMemo(() => {
		const needle = value.trim().toLowerCase();
		const out: Suggestion[] = [];
		for (const s of suggestions) {
			if (
				!needle ||
				s.value.toLowerCase().includes(needle) ||
				s.label?.toLowerCase().includes(needle)
			) {
				out.push(s);
				if (out.length >= max) break;
			}
		}
		return out;
	}, [suggestions, value, max]);

	return (
		<>
			<Input
				{...props}
				type="text"
				list={listId}
				autoComplete="off"
				spellCheck={false}
				value={value}
				onChange={(e) => onChange(e.target.value)}
			/>
			<datalist id={listId}>
				{shown.map((s) => (
					<option key={s.value} value={s.value} label={s.label} />
				))}
			</datalist>
		</>
	);
}
