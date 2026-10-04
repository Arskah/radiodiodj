import type { APIRoute } from "astro";

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Serves the RFC 9116 `security.txt`. Generated rather than static so that
 * `Expires` moves forward with every deploy. See `docs/website.md#contact`.
 * @param context The request context.
 * @param context.site The configured site origin.
 * @returns The file as plain text.
 */
export const GET: APIRoute = ({ site }) => {
  const expires = new Date(Date.now() + 300 * DAY_MS);
  expires.setUTCHours(0, 0, 0, 0);

  const body = [
    "Contact: mailto:security@radiodiodj.org",
    `Expires: ${expires.toISOString()}`,
    "Preferred-Languages: en, fi",
    `Canonical: ${new URL("/.well-known/security.txt", site).href}`,
    "Policy: https://github.com/Arskah/radiodiodj/blob/main/SECURITY.md",
    "",
  ].join("\n");

  return new Response(body, {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
};
