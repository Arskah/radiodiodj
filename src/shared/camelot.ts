/**
 * Camelot codes for the keys the analysis pass measures.
 *
 * The library stores a note name, because that is the vocabulary a tagger writes
 * into a file's own key field and so the one that keeps a measured key and a
 * tagged one comparable. A Camelot code is the same key under a DJ's mixing
 * convention — the wheel's neighbours are the keys that mix well — so it is
 * derived here for display rather than stored twice.
 *
 * The two notations are bijective over all twenty-four keys, so nothing is lost
 * either way. What decides which one is authoritative is that `audio_measure`
 * has no business knowing what a Camelot wheel is.
 */
const CAMELOT: Record<string, string> = {
  // Majors, the wheel's B ring, ascending by fifths from B.
  B: "1B",
  "F#": "2B",
  Db: "3B",
  Ab: "4B",
  Eb: "5B",
  Bb: "6B",
  F: "7B",
  C: "8B",
  G: "9B",
  D: "10B",
  A: "11B",
  E: "12B",
  // Minors, the A ring. Each is the relative minor of the major sharing its
  // number, which is what makes a number the unit of a harmonic mix.
  "G#m": "1A",
  "D#m": "2A",
  Bbm: "3A",
  Fm: "4A",
  Cm: "5A",
  Gm: "6A",
  Dm: "7A",
  Am: "8A",
  Em: "9A",
  Bm: "10A",
  "F#m": "11A",
  "C#m": "12A",
};

/**
 * The Camelot code for a note name, or `null` for anything this map does not
 * cover.
 *
 * A `null` is not an error. `initial_key` carries whatever a tagger wrote —
 * `TKEY` holds a note name by the spec and a Camelot code in practice — so a
 * lookup that misses simply means the string was not one of the twenty-four
 * spellings the measurement produces.
 */
export function camelotOf(key: string | null | undefined): string | null {
  if (!key) return null;
  return CAMELOT[key.trim()] ?? null;
}

/**
 * A measured key as it reads to an operator: the note name, with its Camelot
 * code after it when there is one.
 */
export function formatKey(key: string | null | undefined): string | null {
  if (!key || !key.trim()) return null;
  const name = key.trim();
  const camelot = camelotOf(name);
  return camelot ? `${name} (${camelot})` : name;
}
