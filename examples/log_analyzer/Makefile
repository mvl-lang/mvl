SAMPLE ?= sample.log

.PHONY: run build check

build:
	mvl build main.mvl

check:
	mvl check main.mvl

run: $(SAMPLE)
	mvl run main.mvl -- --file $(SAMPLE)

$(SAMPLE):
	@printf 'INFO  starting up\nWARN  disk at 80%%\nERROR disk full\nINFO  shutdown\n' > $@
