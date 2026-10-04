import type { APIRoute } from "astro";
import { updateManifest } from "../lib/release";

/**
 * Serves the manifest the app's updater checks. See `docs/website.md#updates`.
 * @returns The manifest as JSON.
 */
export const GET: APIRoute = async () =>
  new Response(JSON.stringify(await updateManifest(), null, 2), {
    headers: { "Content-Type": "application/json; charset=utf-8" },
  });
