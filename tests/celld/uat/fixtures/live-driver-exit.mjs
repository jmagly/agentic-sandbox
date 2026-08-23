const exitCode = Number(process.argv[2]);
if (![3, 4].includes(exitCode)) throw new Error("fixture exit code must be 3 or 4");
process.stderr.write(exitCode === 4 ? "typed cleanup residue\n" : "ordinary driver error\n");
process.exitCode = exitCode;
