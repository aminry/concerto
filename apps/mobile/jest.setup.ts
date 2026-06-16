// Jest setup (Task 508). `@testing-library/react-native` auto-registers its
// matchers (`toBeOnTheScreen`, etc.) on import since v12.4 — importing it here
// makes them available to every spec without per-file boilerplate.
import "@testing-library/react-native";
