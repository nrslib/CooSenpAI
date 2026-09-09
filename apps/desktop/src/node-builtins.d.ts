declare module "node:fs" {
  export interface Dirent {
    readonly name: string;
    isDirectory(): boolean;
  }

  export function readFileSync(path: string | URL, encoding: "utf8"): string;
  export function readdirSync(path: string, options: { readonly withFileTypes: true }): Dirent[];
}
