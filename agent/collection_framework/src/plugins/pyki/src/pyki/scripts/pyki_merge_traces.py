# This script is a wrapper for ChromeTraceEvent.merge_traces. We use independent subprocess
# to avoid GIL problem.
import gzip
import argparse
import shutil
from pyki.profiling.chrome_trace import ChromeTraceEvent


def merge_files(source, target):
    sources = source.split(",")
    if len(sources) == 1:
        # no need to merge
        if target.endswith(".gz") and not sources[0].endswith(".gz"):
            # use b mode to prevent illegal character
            with open(sources[0], "rb") as fin:
                with gzip.open(target, "wb") as fout:
                    fout.writelines(fin)
        else:
            shutil.copy(sources[0], target)
    else:
        traces = [ChromeTraceEvent.read_from_file(source) for source in sources]
        merged = ChromeTraceEvent.merge_traces(traces)
        merged.write_to_file(target)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("source", help='source files to merge, seperated by ","', type=str)
    parser.add_argument("target", help="target file to write", type=str)
    args = parser.parse_args()
    merge_files(args.source, args.target)


if __name__ == "__main__":
    main()
