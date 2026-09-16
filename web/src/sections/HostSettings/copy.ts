// The words for each host setting, keyed by the registry id the host sends.
//
// The host owns which settings exist; this file owns how they read. A setting the host knows and
// this console does not falls back to the host's English `title` and shows no hint, so a newer
// host never renders an unlabelled row.
import type { SettingGroup } from "@/api/gen/model";
import { m } from "@/paraglide/messages";

type Copy = {
	label: () => string;
	hint?: () => string;
	/** Enum option labels, keyed by the canonical value. */
	options?: Record<string, () => string>;
};

export const SETTING_COPY: Record<string, Copy> = {
	gamestream: {
		label: m.setting_gamestream,
		hint: m.setting_gamestream_hint,
	},
	webtransport: {
		label: m.setting_webtransport,
		hint: m.setting_webtransport_hint,
	},
	clipboard: {
		label: m.setting_clipboard,
		hint: m.setting_clipboard_hint,
		options: {
			off: m.common_off,
			text: m.setting_clipboard_text,
			files: m.setting_clipboard_files,
		},
	},
	host_name: {
		label: m.setting_host_name,
		hint: m.setting_host_name_hint,
	},
	ten_bit: {
		label: m.setting_ten_bit,
		hint: m.setting_ten_bit_hint,
	},
	chroma_444: {
		label: m.setting_chroma_444,
		hint: m.setting_chroma_444_hint,
	},
	max_fps: {
		label: m.setting_max_fps,
		hint: m.setting_max_fps_hint,
	},
	audio_output_mode: {
		label: m.setting_audio_output_mode,
		options: {
			client_only: m.setting_audio_output_mode_client_only,
			host_and_client: m.setting_audio_output_mode_host_and_client,
			follow_default: m.setting_audio_output_mode_follow_default,
		},
	},
	audio_voice_chat: {
		label: m.setting_audio_voice_chat,
		hint: m.setting_audio_voice_chat_hint,
		options: {
			stream: m.setting_audio_voice_chat_stream,
			host: m.setting_audio_voice_chat_host,
		},
	},
	audio_voice_apps: {
		label: m.setting_audio_voice_apps,
		hint: m.setting_audio_voice_apps_hint,
	},
};

export const GROUP_LABEL: Record<SettingGroup, () => string> = {
	streaming: m.settings_group_streaming,
	video: m.settings_group_video,
	audio: m.settings_group_audio,
	input: m.settings_group_input,
	network: m.settings_group_network,
	game_mode: m.settings_group_game_mode,
	session: m.settings_group_session,
	system: m.settings_group_system,
};
