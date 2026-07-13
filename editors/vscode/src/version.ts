import pkg from "../package.json";

export const PINNED_SERVER_VERSION: string = pkg.version;

export function parseServerVersion(stdout: string): string | null {
  const m = /^compact-analyzer (\d+\.\d+\.\d+\S*)\s*$/m.exec(stdout.trim());
  // A successful match guarantees capture group 1; the `!` satisfies
  // `noUncheckedIndexedAccess`, which otherwise widens `m[1]` to `undefined`.
  return m ? m[1]! : null;
}

function split(v: string): { major: number; minor: number } | null {
  const m = /^(\d+)\.(\d+)\.\d+/.exec(v);
  return m ? { major: Number(m[1]), minor: Number(m[2]) } : null;
}

export function isCompatible(serverVersion: string, pinned: string = PINNED_SERVER_VERSION): boolean {
  const s = split(serverVersion);
  const p = split(pinned);
  if (!s || !p) return false;
  // Pre-1.0 the minor is the breaking axis; from 1.0 the major is.
  return p.major === 0 ? s.major === 0 && s.minor === p.minor : s.major === p.major;
}
