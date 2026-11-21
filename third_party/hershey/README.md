# Hershey fonts (third-party data)

`futural.jhf` is the Hershey "Roman Simplex" font (Hershey glyphs mapped
to ASCII 32 to 127, one glyph per line in James Hurt's format), unmodified.
`crates/terse-mame-crt` draws terminal text with it, as strokes of the beam.

- Fetched from https://github.com/kamalmostafa/hershey-fonts, file
  `hershey-fonts/futural.jhf`, commit
  `1356bf2f83d380fcef68c887e88675eb9d445d86`. That repository's library
  code is GPL; only the font data file, which is under the Hershey
  distribution's own terms below, was taken. Its notes give the data's
  source as http://emergent.unpythonic.net/software/hershey.

## Terms (from the original Usenet distribution's `hershey.txt`)

    USE RESTRICTION:
    	This distribution of the Hershey Fonts may be used by anyone for
    	any purpose, commercial or otherwise, providing that:
    		1. The following acknowledgements must be distributed with
    			the font data:
    			- The Hershey Fonts were originally created by Dr.
    				A. V. Hershey while working at the U. S.
    				National Bureau of Standards.
    			- The format of the Font data in this distribution
    				was originally created by
    					James Hurt
    					Cognition, Inc.
    					900 Technology Park Drive
    					Billerica, MA 01821
    					(mit-eddie!ci-dandelion!hurt)
    		2. The font data in this distribution may be converted into
    			any other format *EXCEPT* the format distributed by
    			the U.S. NTIS (which organization holds the rights
    			to the distribution and use of the font data in that
    			particular format). Not that anybody would really
    			*want* to use their format... each point is described
    			in eight bytes as "xxx yyy:", where xxx and yyy are
    			the coordinate values as ASCII numbers.

## Acknowledgements

- The Hershey Fonts were originally created by Dr. A. V. Hershey while
  working at the U. S. National Bureau of Standards.
- The format of the Font data in this distribution was originally created
  by James Hurt, Cognition, Inc., 900 Technology Park Drive, Billerica, MA
  01821 (mit-eddie!ci-dandelion!hurt).
