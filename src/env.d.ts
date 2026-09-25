declare module "*.css";
declare module "*.svg" {
  const url: string;
  export default url;
}
declare module "*.svelte" {
  import type { Component } from "svelte";
  const component: Component;
  export default component;
}

type ContentType = "music" | "commercial" | "jingle";
type SortColumn =
  | "title"
  | "artist"
  | "album"
  | "play_count"
  /** Album order: album, then disc, then track. Not a bare number sort. */
  | "track_no";
type SortDir = "asc" | "desc";
