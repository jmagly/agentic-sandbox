.PHONY: install test lint format type-check clean build publish help

help:
	@echo "Available targets:"
	@echo "  install      - Install package with dev dependencies"
	@echo "  test         - Run test suite with coverage"
	@echo "  test-quick   - Run tests without coverage"
	@echo "  lint         - Run linting checks"
	@echo "  format       - Format code with black"
	@echo "  type-check   - Run mypy type checker"
	@echo "  clean        - Remove build artifacts"
	@echo "  build        - Build distribution packages"
	@echo "  publish      - Publish to PyPI (requires credentials)"

install:
	pip install -e ".[dev]"

test:
	pytest --cov=agentic_sandbox --cov-report=term-missing --cov-report=html

test-quick:
	pytest -v

lint:
	ruff check agentic_sandbox tests

format:
	black agentic_sandbox tests

type-check:
	mypy agentic_sandbox

clean:
	rm -rf build/ dist/ *.egg-info
	rm -rf .pytest_cache .mypy_cache .ruff_cache
	rm -rf htmlcov .coverage
	find . -type d -name __pycache__ -exec rm -rf {} +
	find . -type f -name "*.pyc" -delete

build: clean
	python -m build

publish: build
	python -m twine upload dist/*
