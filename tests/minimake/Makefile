# A small Makefile for examples/real/minimake.yx
OUT = /tmp/yerux_minimake_test
CAT = /bin/cat

.PHONY: all clean info

all: $(OUT)/app info

info:
	@echo "building into $(OUT)"
	-/bin/false
	@echo "an ignored failure above"

$(OUT)/dir:
	/bin/mkdir -p $(OUT)
	@/usr/bin/touch $@

$(OUT)/a.txt: tests/minimake/a.src $(OUT)/dir
	$(CAT) $< > $@

$(OUT)/b.txt: tests/minimake/b.src $(OUT)/dir
	$(CAT) $< > $@

$(OUT)/app: $(OUT)/a.txt $(OUT)/b.txt
	$(CAT) $^ > $@
	@echo "linked $@ from $^"

clean:
	/bin/rm -rf $(OUT)
