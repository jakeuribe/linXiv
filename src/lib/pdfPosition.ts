export interface PdfPosition {
  page: number;
  /** Fractional distance from the top of the page (0–1). */
  offset: number;
}

const PDF_POSITION_PREFIX = "linxiv-pdf-position:";

type PositionStorage = Pick<Storage, "getItem" | "setItem">;

export function pdfPositionStorageKey(sourceId: string, version: number): string {
  return `${PDF_POSITION_PREFIX}${encodeURIComponent(sourceId)}:v${version}`;
}

export function parsePdfPosition(raw: string | null): PdfPosition | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as unknown;
    if (!value || typeof value !== "object") return null;
    const candidate = value as Record<string, unknown>;
    if (
      typeof candidate.page !== "number" ||
      !Number.isFinite(candidate.page) ||
      candidate.page < 1 ||
      typeof candidate.offset !== "number" ||
      !Number.isFinite(candidate.offset)
    ) {
      return null;
    }
    return {
      page: Math.floor(candidate.page),
      offset: Math.min(1, Math.max(0, candidate.offset)),
    };
  } catch {
    return null;
  }
}

export function readPdfPosition(
  sourceId: string,
  version: number,
  storage: PositionStorage = localStorage,
): PdfPosition | null {
  try {
    return parsePdfPosition(storage.getItem(pdfPositionStorageKey(sourceId, version)));
  } catch {
    // Storage can be unavailable in privacy-restricted webviews. The reader
    // should remain usable even when its optional place-saving cannot run.
    return null;
  }
}

export function writePdfPosition(
  sourceId: string,
  version: number,
  position: PdfPosition,
  storage: PositionStorage = localStorage,
): void {
  try {
    storage.setItem(
      pdfPositionStorageKey(sourceId, version),
      JSON.stringify({
        page: Math.max(1, Math.floor(position.page)),
        offset: Math.min(1, Math.max(0, position.offset)),
      }),
    );
  } catch {
    // See readPdfPosition: persistence failure must not break PDF scrolling.
  }
}
