// The post-login redirect target. The gate hands a signed-in visitor on from /login to this path,
// so it is what stands between a crafted `?next=` link and an open redirect.
import { describe, expect, test } from "bun:test";
import { safeNextPath } from "./auth";

describe("safeNextPath", () => {
	test("keeps a same-origin path with its query and hash", () => {
		expect(safeNextPath("/host?tab=power#top")).toBe("/host?tab=power#top");
	});

	test("falls back to / when unset or empty", () => {
		expect(safeNextPath(undefined)).toBe("/");
		expect(safeNextPath("")).toBe("/");
	});

	test("refuses an off-origin target", () => {
		for (const evil of [
			"https://evil.com/",
			"//evil.com/x",
			"/\\evil.com",
			"\\\\evil.com",
		]) {
			expect(safeNextPath(evil)).toBe("/");
		}
	});

	test("refuses the login page, which would bounce through the gate", () => {
		expect(safeNextPath("/login")).toBe("/");
		expect(safeNextPath("/login?next=%2Flogin")).toBe("/");
	});
});
