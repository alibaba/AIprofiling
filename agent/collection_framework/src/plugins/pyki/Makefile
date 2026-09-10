.DEFAULT_GOAL := package
# configable
PYTHON ?= python3
# internal usage
PIP = $(PYTHON) -m pip
TEST_ENV = HF_ENDPOINT=https://hf-mirror.com
TEST_ARGS = -s -v --continue-on-collection-errors --timeout=300
CURRENT_CWD := $(PWD)

package:
	$(PYTHON) -m build --wheel --no-isolation

clean:
	rm -rf dist/
	rm -rf build/
	rm -rf *.egg-info/
	rm -rf tests_shadowed
	@(cd src/pyki/native/rust && cargo clean)


install:
	$(MAKE) clean
	@(cd src/pyki/native/rust && cargo build --release)
	$(MAKE) package
	$(PIP) install --force-reinstall $$(ls ./dist/*.whl 2>/dev/null | head -n 1)

install_dev:
	$(MAKE) clean
	$(MAKE) uninstall
	@(cd src/pyki/native/rust && cargo build)
	PYKI_DEBUG=1 PYKI_SHADOW=0 $(PIP) install -e .

install_cov:
	$(MAKE) clean
	$(MAKE) uninstall
	@(cd src/pyki/native/rust && cargo build --release)
	PYKI_COVERAGE=1 PYKI_SHADOW=0 $(PIP) install -e .

uninstall:
	$(PIP) uninstall -y pyki

test:
	$(MAKE) shadow_test
	$(TEST_ENV) $(PYTHON) -m pytest $(TEST_ARGS) --reruns=2 tests_shadowed

unit_test:
	$(MAKE) shadow_test
	$(TEST_ENV) $(PYTHON) -m pytest  tests_shadowed/testcases/unit_test

unit_test_dev:
	$(TEST_ENV) $(PYTHON) -m pytest  tests/testcases/unit_test

test_dev:
	$(TEST_ENV) $(PYTHON) -m pytest $(TEST_ARGS) --reruns=2 tests

test_cov:
	$(PYTHON) -m coverage erase
	find . -name "*.gcda" -delete 2>/dev/null || true
	rm -rf htmlcov

	$(TEST_ENV) \
	PYKI_COVERAGE=1 \
	$(PYTHON) -m coverage run --source="$(CURRENT_CWD)/src" -m pytest $(TEST_ARGS) tests  || true

	$(PYTHON) -m coverage combine
	$(PYTHON) -m coverage html
	$(PYTHON) -m gcovr --html-details -o htmlcov/cpp_coverage.html

	echo "python coverage"
	$(PYTHON) -m coverage report
	echo "c coverage"
	$(PYTHON) -m gcovr

reinstall:
	$(MAKE) clean
	$(MAKE) uninstall
	$(MAKE) install

shadow_test:
	$(PYTHON) shadow_test.py

.PHONY: package clean install uninstall reinstall test shadow_test install_dev test_dev install_cov test_cov
