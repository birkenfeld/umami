Unified Mechanism for Acquisition of Measured Intensity (UMAMI)
===============================================================

UMAMI is a data acquisition backend for neutron detectors.  It implements a
"modular pipeline" approach which combines detector-specific backends with
configurable processing steps.


How it works
------------

One UMAMI process is configured by a config file that configures and starts
several components (many in separate threads) that act as a data pipeline:

* Input modules: connect to a detector/data source and reads events into the
  common internal event structure, dumping raw data to disk if wanted
* Input recipes: transform event data to assign logical meaning to event types
  and locations
* Sorters: merge events from all modules into a single data stream, sorted by
  absolute timestamp
* Postprocess recipes: run transformations only possible looking at the sorted
  data stream, e.g. assigning a relative timestamp to events
* Histogram: automatic histogramming of events in 2- or 3-dimensional arrays
* Outputs: save the processed event stream in desired formats


Authors
-------

UMAMI is brought to you by

* Georg Brandl <g.brandl@fz-juelich.de>
* Alexander Zaft <a.zaft@fz-juelich.de>
