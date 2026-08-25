const exitCode = Number(process.argv[2]);
if (![3, 4].includes(exitCode)) throw new Error("fixture exit code must be 3 or 4");
process.stdout.write("driver stdout marker");
process.stderr.write("driver stderr marker");
process.exitCode = exitCode;
