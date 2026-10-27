anchor_bash() { local value="$1"; printf "%s\n" "$((value+41))"; }
caller_bash() { anchor_bash 1; }
