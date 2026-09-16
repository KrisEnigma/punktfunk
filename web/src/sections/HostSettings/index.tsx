// Container for Host → Settings: the settings query, one-setting writes, and the app list the
// voice-chat picker suggests from. Everything visual is in `view.tsx`.
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "@unom/ui/toast";
import { type FC, useState } from "react";
import {
	getGetHostSettingsQueryKey,
	patchHostSettings,
	useGetHostSettings,
	useGetPlayingApps,
} from "@/api/gen/host/host";
import { apiErrorMessage } from "@/lib/errors";
import { useLocale } from "@/lib/i18n";
import { m } from "@/paraglide/messages";
import { HostSettingsView } from "./view";

export const SectionHostSettings: FC = () => {
	useLocale();
	const qc = useQueryClient();
	const state = useGetHostSettings();
	const hasVoiceApps = !!state.data?.settings.some(
		(s) => s.id === "audio_voice_apps",
	);
	const apps = useGetPlayingApps({
		query: { enabled: hasVoiceApps, refetchInterval: 15_000 },
	});
	const [pending, setPending] = useState<ReadonlySet<string>>(new Set());

	const settle = (id: string, busy: boolean) =>
		setPending((prev) => {
			const next = new Set(prev);
			if (busy) next.add(id);
			else next.delete(id);
			return next;
		});

	// One key per request: the answer is the whole new state, so it replaces the cache outright.
	const onSet = async (id: string, value: unknown) => {
		settle(id, true);
		try {
			const next = await patchHostSettings({ [id]: value });
			qc.setQueryData(getGetHostSettingsQueryKey(), next);
		} catch (e) {
			toast.error(apiErrorMessage(e) ?? m.host_settings_save_failed());
			qc.invalidateQueries({ queryKey: getGetHostSettingsQueryKey() });
		} finally {
			settle(id, false);
		}
	};

	return (
		<HostSettingsView
			state={state}
			pending={pending}
			onSet={onSet}
			playingApps={apps.data?.apps}
		/>
	);
};
