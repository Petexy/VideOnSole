# English, as America writes it.
#
# An overlay rather than a catalog: every message not written here is answered
# out of `en-GB.ftl`, which is the English this application is written in.
# Only what the two Englishes really disagree about belongs here, and
# `Catalog::validate` fails a file that copies a message across unchanged or
# writes one no other catalog has. See docs/localization.md.

# The month goes first. The time after it is the session's own clock, which is
# a setting rather than a language — see `lxb_toolkit::i18n::time_of_day`.
file-date = { $month } { $day }, { $year }, { $time }
