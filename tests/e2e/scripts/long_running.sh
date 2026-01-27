#!/bin/bash
# Reads stdin lines and echoes them back until EOF or killed
while IFS= read -r line; do
    echo "GOT: $line"
done
