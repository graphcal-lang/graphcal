import { readFile, mkdir, copyFile, rm } from "node:fs/promises";
const root = new URL("../../", import.meta.url);
const destination = new URL("public/examples/", import.meta.url);
const catalog = JSON.parse(
  await readFile(new URL("examples/catalog.json", import.meta.url), "utf8"),
);
await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
for (const example of catalog) {
  if (!/^[a-z][a-z0-9-]*$/.test(example.id)) throw new Error("Unsafe catalog ID");
  await copyFile(new URL(example.source_path, root), new URL(`${example.id}.gcl`, destination));
}
