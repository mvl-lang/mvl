"use strict";

const nodeBinding = require("node-gyp-build")(__dirname + "/../..");

/** @type {import("./index.d.ts")} */
module.exports = nodeBinding;
