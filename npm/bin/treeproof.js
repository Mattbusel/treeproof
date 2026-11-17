#!/usr/bin/env node
"use strict";
const { spawnSync } = require("child_process");
const { install } = require("./install.js");

install()
  .then((bin) => {
    const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
    if (r.error) {
      console.error(`treeproof: ${r.error.message}`);
      process.exit(1);
    }
    process.exit(r.status === null ? 1 : r.status);
  })
  .catch((e) => {
    console.error(e.message);
    process.exit(1);
  });
