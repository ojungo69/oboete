import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from m3 import its, shares


def test_a_claim_is_the_decision_whose_quote_it_shares():
    # One prompt, two decisions (d105, d106): each claim is the one its quote shares text with.
    both = 'じゃあCCSを完全削除して。あと、fccをxai oauthに対応させたい。'
    assert shares('CCSを完全削除して', 'じゃあCCSを完全削除して。')
    assert not shares('fccをxai oauthに対応させたい', 'じゃあCCSを完全削除して。')
    assert shares(both, 'あと、fccをxai oauthに対応させたい。')
    # Whitespace aside, and the whole of a quote shorter than the stretch.
    assert shares('tabs in every file', 'tabs\nin every  file of it')
    assert shares('１', '１')
    assert not shares('１', '２')
    # An empty quote is no one's.
    assert not shares('', 'じゃあCCSを完全削除して。')
    assert not shares(' ', 'x')


def test_a_record_of_two_decisions_gives_each_its_own_claims():
    # d105 and d106 share one prompt: a claim is the decision whose quote it shares, and one that
    # shares neither is both decisions' claim, as a record's claims all were before.
    quote_of = {'d105': 'じゃあCCSを完全削除して。', 'd106': 'あと、fccをxai oauthに対応させたい。'}
    on = ['d105', 'd106']
    assert its('d105', ['CCSを完全削除して'], on, quote_of)
    assert not its('d106', ['CCSを完全削除して'], on, quote_of)
    assert its('d106', ['fccをxai oauthに対応させたい'], on, quote_of)
    assert not its('d105', ['fccをxai oauthに対応させたい'], on, quote_of)
    assert its('d105', ['やって'], on, quote_of) and its('d106', ['やって'], on, quote_of)
    # Alone on its record, any claim there is the item's. The other end of an accepted proposal
    # joins the record's items: a quote of the item there is its own.
    assert its('d105', ['x'], ['d105'], quote_of)
    other_end = quote_of | {'d107': '１'}
    assert not its('d107', ['CCSを完全削除して'], ['d105'], other_end)
    assert its('d107', ['１'], ['d105'], other_end)
