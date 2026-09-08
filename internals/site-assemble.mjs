import { cp, rm } from "node:fs/promises";
const root = new URL("../", import.meta.url);
await cp(new URL("site-root/", root), new URL("site/", root), { recursive: true });
await cp(new URL("site/docs/404.html", root), new URL("site/404.html", root));
await rm(new URL("site/playground/", root), { recursive: true, force: true });
await cp(new URL("web/playground/dist/", root), new URL("site/playground/", root), { recursive: true });
