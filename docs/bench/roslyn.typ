// Compile-speed comparison against Roslyn.
//
//   typst compile docs/bench/roslyn.typ docs/bench/roslyn-{n}.png --ppi 300
//
// 25 runs per (configuration, corpus size). The numbers come from
// `tools/bench-roslyn.sh`, which writes artifacts/bench-roslyn/summary.tsv;
// they are inlined below so that this figure builds without the artifacts
// directory. Rerun the script on your own machine and replace the
// `measurements` block to redraw it.

#set page(width: 15cm, height: auto, margin: (x: 12pt, y: 14pt))
#set text(font: ("New Computer Modern", "Noto Serif CJK JP"), size: 9pt)
#set par(justify: false)

// ---------------------------------------------------------------- data ----

#let machine = [AMD Ryzen AI 9 365 (10 cores / 20 threads), 30 GB RAM, Linux 6.18]
#let versions = [M\# 0.1.0 (release build), Roslyn `csc` 5.0.0 (bundled with .NET SDK 10.0.110); reference assemblies from Unity 2022.3.22f1 and the VRChat Worlds SDK]

// (lines, files, (mean, sd) per configuration), n = 25 runs each
// configurations: mensharp, mensharp-check, roslyn-warm, roslyn-cold
#let runs = 25
#let measurements = (
  (lines: 12, files: 1, mensharp: (46.8, 6.3), check: (27.6, 2.3), warm: (74.3, 10.4), cold: (434.0, 26.4)),
  (lines: 1298, files: 22, mensharp: (51.9, 7.0), check: (33.5, 2.4), warm: (86.1, 11.7), cold: (596.9, 38.4)),
  (lines: 2596, files: 44, mensharp: (54.2, 8.7), check: (34.4, 2.2), warm: (93.4, 9.9), cold: (629.7, 44.0)),
  (lines: 5192, files: 88, mensharp: (59.0, 8.3), check: (38.4, 3.0), warm: (104.0, 8.7), cold: (634.9, 32.4)),
  (lines: 10384, files: 176, mensharp: (70.6, 8.4), check: (44.2, 2.8), warm: (135.9, 19.2), cold: (707.6, 34.3)),
  (lines: 20768, files: 352, mensharp: (85.2, 7.4), check: (55.6, 4.2), warm: (187.6, 16.3), cold: (742.9, 42.6)),
  (lines: 41536, files: 704, mensharp: (123.7, 7.0), check: (76.9, 4.7), warm: (290.9, 23.0), cold: (905.1, 47.8)),
)

// `str` drops trailing zeros, which makes a column of ratios ragged
#let fixed(value, digits) = {
  let rendered = str(calc.round(value, digits: digits))
  if digits == 0 { return rendered }
  if rendered.position(".") == none { rendered = rendered + "." }
  let places = rendered.len() - rendered.position(".") - 1
  rendered + "0" * (digits - places)
}

#let thousands(value) = {
  let digits = str(value).clusters().rev()
  digits.enumerate()
    .map(((index, digit)) => if index > 0 and calc.rem(index, 3) == 0 { digit + "," } else { digit })
    .rev()
    .join()
}

#let ink = luma(15%)
#let colours = (
  mensharp: rgb("#26374d"),
  warm: rgb("#8f9bad"),
  cold: rgb("#dbdfe6"),
)

// --------------------------------------------------------------- chart ----

// A grouped bar chart on a linear axis, with +/- whiskers and the value
// printed above each bar. `series` is a list of (label, colour, values),
// where each `values` entry is a (mean, sd) pair, one per group.
#let bars(
  groups: (),
  series: (),
  ymax: 100.0,
  yticks: (),
  pw: 11.6cm,
  ph: 5.6cm,
  bar: 10pt,
  gap: 2pt,
  pad-left: 34pt,
  pad-bottom: 36pt,
  pad-top: 14pt,
  ylabel: none,
  xlabel: none,
  legend: true,
  values: true,
  digits: 0,
) = {
  let position = v => ph * (1.0 - calc.max(v, 0.0) / ymax)

  let count = series.len()
  let group-width = pw / groups.len()
  let cluster = count * bar + (count - 1) * gap

  box(width: pw + pad-left, height: ph + pad-bottom + pad-top, {
    if legend {
      place(dx: pad-left, dy: 0pt, {
        set text(size: 7.5pt)
        stack(dir: ltr, spacing: 12pt, ..series.map(entry => box(
          baseline: 1pt,
          stack(
            dir: ltr,
            spacing: 4pt,
            rect(width: 7pt, height: 7pt, fill: entry.at(1), stroke: 0.4pt + ink),
            entry.at(0),
          ),
        )))
      })
    }

    let top = pad-top

    // horizontal grid and y ticks
    for tick in yticks {
      let y = top + position(tick)
      place(dx: pad-left, dy: y, line(length: pw, stroke: 0.3pt + luma(88%)))
      place(dx: pad-left - 4pt, dy: y, line(length: 3pt, stroke: 0.5pt + ink))
      place(
        dx: pad-left - 30pt,
        dy: y - 4.2pt,
        box(width: 25pt, align(right, text(size: 7pt, [#tick]))),
      )
    }

    // bars
    for (index, group) in groups.enumerate() {
      let left = pad-left + index * group-width + (group-width - cluster) / 2
      for (offset, entry) in series.enumerate() {
        let (mean, sd) = entry.at(2).at(index)
        let x = left + offset * (bar + gap)
        let y = top + position(mean)
        place(
          dx: x,
          dy: y,
          rect(width: bar, height: top + ph - y, fill: entry.at(1), stroke: 0.4pt + ink),
        )
        let high = top + position(mean + sd)
        let low = top + position(mean - sd)
        place(dx: x + bar / 2, dy: high, line(length: low - high, angle: 90deg, stroke: 0.5pt + ink))
        for cap in (high, low) {
          place(dx: x + bar / 2 - 2pt, dy: cap, line(length: 4pt, stroke: 0.5pt + ink))
        }
        if values {
          place(
            dx: x + bar / 2 - 15pt,
            dy: high - 10pt,
            box(width: 30pt, align(center, text(size: 6.5pt, fixed(mean, digits)))),
          )
        }
      }
      place(
        dx: pad-left + index * group-width,
        dy: top + ph + 4pt,
        box(width: group-width, align(center, text(size: 7.5pt, group))),
      )
    }

    // axes
    place(dx: pad-left, dy: top + ph, line(length: pw, stroke: 0.6pt + ink))
    place(dx: pad-left, dy: top, line(length: ph, angle: 90deg, stroke: 0.6pt + ink))

    // the label is laid out horizontally and then turned a quarter turn in
    // place, so it is centred on the axis rather than wrapped into a column
    if ylabel != none {
      place(
        dx: 9pt - ph / 2,
        dy: top + ph / 2 - 5pt,
        rotate(-90deg, box(width: ph, height: 10pt, align(center, text(size: 8pt, ylabel)))),
      )
    }
    if xlabel != none {
      place(
        dx: pad-left,
        dy: top + ph + 22pt,
        box(width: pw, align(center, text(size: 8pt, xlabel))),
      )
    }
  })
}

// ------------------------------------------------------------- figure 1 ----

#let sized = measurements.slice(1)
#let group-labels = sized.map(row => [#fixed(row.lines / 1000, 1)k \ #text(size: 6.5pt)[(#row.files files)]])

#align(center)[
  #text(size: 10pt, weight: "bold")[Time to compile the same C\# sources]
  #v(2pt)
  #text(size: 8pt)[M\# emits Udon assembly, Roslyn emits IL. Whiskers are #sym.plus.minus 1 SD (n = #runs)]
]
#v(6pt)

#bars(
  groups: group-labels,
  series: (
    ([M\# (always cold)], colours.mensharp, sized.map(row => row.mensharp)),
    ([Roslyn (warm: build server)], colours.warm, sized.map(row => row.warm)),
    ([Roslyn (cold: fresh process)], colours.cold, sized.map(row => row.cold)),
  ),
  ymax: 1000.0,
  yticks: (0, 200, 400, 600, 800, 1000),
  ylabel: [wall-clock time (ms)],
  xlabel: [corpus size (lines / files)],
)

#v(4pt)
#block(width: 100%, inset: (x: 34pt), text(size: 7.5pt)[
  *Times slower than M\#* #h(6pt)
  #table(
    columns: 7,
    stroke: none,
    align: (left, ..(right,) * 6),
    inset: (x: 5pt, y: 1.5pt),
    [corpus], ..sized.map(row => [#fixed(row.lines / 1000, 1)k]),
    [warm], ..sized.map(row => [#fixed(row.warm.at(0) / row.mensharp.at(0), 2)#sym.times]),
    [cold], ..sized.map(row => [#fixed(row.cold.at(0) / row.mensharp.at(0), 1)#sym.times]),
  )
])

#pagebreak()

// ------------------------------------------------------------- figure 2 ----

// Least squares fit of time = intercept + slope * lines over the six real
// corpus sizes: `slope` is what one more line of C# costs. The fixed cost is
// not taken from the intercept but measured directly, with the twelve-line
// corpus -- for the cold configuration the curve is too concave near the
// origin for an extrapolated intercept to mean anything.
#let fit(key) = {
  let xs = sized.map(row => row.lines)
  let ys = sized.map(row => row.at(key).at(0))
  let n = xs.len()
  let mx = xs.sum() / n
  let my = ys.sum() / n
  let sxx = xs.map(x => calc.pow(x - mx, 2)).sum()
  let sxy = xs.zip(ys).map(p => (p.at(0) - mx) * (p.at(1) - my)).sum()
  let slope = sxy / sxx
  let intercept = my - slope * mx
  let residuals = xs.zip(ys).map(p => p.at(1) - (intercept + slope * p.at(0)))
  let sse = residuals.map(r => r * r).sum()
  let sigma = calc.sqrt(sse / (n - 2))
  (
    intercept: intercept,
    intercept-se: sigma * calc.sqrt(1.0 / n + mx * mx / sxx),
    slope: slope,
    slope-se: sigma / calc.sqrt(sxx),
  )
}

#let fits = (
  mensharp: fit("mensharp"),
  warm: fit("warm"),
  cold: fit("cold"),
)

// thousands of lines per second, with the slope's error carried through the
// reciprocal
#let throughput(f) = {
  let rate = 1.0 / f.slope
  (rate, rate * f.slope-se / f.slope)
}

#align(center)[
  #text(size: 10pt, weight: "bold")[What the fixed cost is, and how fast each line goes through]
  #v(2pt)
  #text(size: 8pt)[
    Left: the cost before any real source is read, measured on a 12-line,
    one-file corpus (#sym.plus.minus 1 SD). #h(4pt)
    Right: throughput net of that fixed cost, from the slope of a least-squares fit
    #box[time = a + b #sym.times lines] over the six corpus sizes,
    1,298 -- 41,536 lines (#sym.plus.minus 1 SE)
  ]
]
#v(10pt)

#grid(
  columns: (1fr, 1fr),
  gutter: 10pt,
  bars(
    groups: ([M\#], [Roslyn \ warm], [Roslyn \ cold]),
    series: (
      ([fixed cost], luma(45%), (
        measurements.at(0).mensharp,
        measurements.at(0).warm,
        measurements.at(0).cold,
      )),
    ),
    ymax: 500.0,
    yticks: (0, 100, 200, 300, 400, 500),
    pw: 4.4cm,
    ph: 4.2cm,
    bar: 16pt,
    pad-top: 4pt,
    legend: false,
    digits: 1,
    ylabel: [fixed cost (ms)],
  ),
  bars(
    groups: ([M\#], [Roslyn \ warm], [Roslyn \ cold]),
    series: (
      ([throughput], luma(45%), (
        throughput(fits.mensharp),
        throughput(fits.warm),
        throughput(fits.cold),
      )),
    ),
    ymax: 600.0,
    yticks: (0, 100, 200, 300, 400, 500, 600),
    pw: 4.4cm,
    ph: 4.2cm,
    bar: 16pt,
    pad-top: 4pt,
    legend: false,
    ylabel: [throughput (k lines / s)],
  ),
)

#v(8pt)
#block(inset: (x: 20pt), text(size: 8.5pt)[
  Take the fixed cost away and M\# gets through
  #strong[#fixed(throughput(fits.mensharp).at(0), 0) thousand lines per second],
  against #fixed(throughput(fits.warm).at(0), 0) for a warm Roslyn and
  #fixed(throughput(fits.cold).at(0), 0) for a cold one. And a line of C\# costs M\#
  more work to begin with: it ends as
  #box[one Udon program per behaviour, written out as three files.]
])

#pagebreak()

// --------------------------------------------------------------- table ----

#align(center)[#text(size: 10pt, weight: "bold")[Measurements (mean #sym.plus.minus SD, ms, n = #runs)]]
#v(8pt)

#let cell(pair) = [#fixed(pair.at(0), 1) #sym.plus.minus #fixed(pair.at(1), 1)]

#align(center, table(
  columns: 6,
  stroke: none,
  align: right,
  inset: (x: 7pt, y: 3.5pt),
  table.hline(stroke: 0.7pt),
  table.header(
    [lines], [files], [M\#], [M\# \ (check only)], [Roslyn \ warm], [Roslyn \ cold],
  ),
  table.hline(stroke: 0.4pt),
  ..measurements
    .map(row => (
      [#thousands(row.lines)], [#thousands(row.files)],
      cell(row.mensharp), cell(row.check), cell(row.warm), cell(row.cold),
    ))
    .flatten(),
  table.hline(stroke: 0.7pt),
))

#v(10pt)
#block(width: 100%, inset: (x: 24pt), text(size: 8pt)[
  *How this was measured*

  - Machine: #machine.
  - Versions: #versions.
  - Both compilers are handed the #strong[same .cs files] and the
    #strong[same nine reference assemblies] (Unity's mscorlib and netstandard
    facade, UnityEngine.CoreModule / PhysicsModule, the M\# runtime assembly and
    four VRChat SDK assemblies). The corpus is `tests/bench-corpus`, scaled by
    duplicating it into fresh namespaces.
  - What is timed is wall-clock time from process start to exit. M\# writes Udon
    programs (`.uasm` / `.meta.json` / `.uprog`), Roslyn writes a `.dll`; both
    really produce their output.
  - Roslyn runs with `-debug- -optimize-` -- its fastest setting -- and with no
    analyzers and no source generators. #strong[Warm] is `-shared`, i.e. through a
    VBCSCompiler build server that is already running, already JIT-compiled and
    already holding the references it read. #strong[Cold] uses no build server: a
    fresh process every run.
  - M\# has no warm mode to report. It keeps no resident server, so
    #strong[every M\# number here is a cold number].
  - All 28 (configuration, corpus size) cells are run once per round, in a random
    order within each round, for #runs rounds -- so no configuration is measured
    only at one point in time, or only ever after one particular neighbour. Three
    warm-up rounds are discarded.
  - "M\# (check only)" stops after parsing, name resolution and type checking: no
    code generation, no output. It is not comparable to Roslyn and is listed only
    as a breakdown of where M\# spends its time.
  - One corpus file is one behaviour, which is one Udon program out of M\#: the
    41,536-line corpus makes it generate and write 704 of them.
  - To reproduce: `tools/bench-roslyn.sh`
])
