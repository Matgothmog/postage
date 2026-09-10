/// `qrcode` ships no TypeScript declarations of its own, and this repo has no
/// `@types/qrcode`. This ambient module declares only the one export the app
/// actually uses — `create()`, the synchronous, pure data-transform QR
/// encoder — not the async canvas/Promise renderers the package also
/// exposes.
declare module "qrcode" {
  export function create(
    text: string,
    options?: { errorCorrectionLevel?: "L" | "M" | "Q" | "H" }
  ): { modules: { readonly size: number; get(row: number, col: number): number } };
}
