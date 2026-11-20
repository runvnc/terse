CC ?= cc
CFLAGS ?= -O2 -Wall -Wextra -std=c11 -D_GNU_SOURCE
OBJS = main.o machine.o cpu.o bus.o blake3.o

tcore: $(OBJS)
	$(CC) $(CFLAGS) -o $@ $(OBJS)

%.o: %.c core.h
	$(CC) $(CFLAGS) -c $<

test: tcore
	./test.sh

clean:
	rm -f tcore $(OBJS)

.PHONY: test clean
